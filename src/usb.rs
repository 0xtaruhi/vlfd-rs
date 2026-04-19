use crate::error::{Error, Result};
use nusb::{
    self, Device, DeviceId, DeviceInfo, Interface, MaybeFuture,
    transfer::{Bulk, In, Out},
};
use std::{
    io::{Read, Write},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::Duration,
};

#[cfg(target_endian = "big")]
compile_error!("vlfd-rs currently supports little-endian hosts only");

const INTERFACE: u8 = 0;
const HOTPLUG_POLL_INTERVAL: Duration = Duration::from_millis(100);
const IO_BUFFER_SIZE: usize = 16 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TransportConfig {
    pub usb_timeout: Duration,
    pub sync_timeout: Duration,
    pub reset_on_open: bool,
    pub clear_halt_on_open: bool,
}

impl Default for TransportConfig {
    fn default() -> Self {
        Self {
            usb_timeout: Duration::from_millis(1_000),
            sync_timeout: Duration::from_secs(1),
            reset_on_open: false,
            clear_halt_on_open: true,
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub enum Endpoint {
    FifoWrite = 0x02,
    Command = 0x04,
    FifoRead = 0x86,
    Sync = 0x88,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HotplugEventKind {
    Arrived,
    Left,
}

#[derive(Debug, Clone)]
pub struct HotplugDeviceInfo {
    pub bus_number: u8,
    pub address: u8,
    pub port_numbers: Vec<u8>,
    pub vendor_id: Option<u16>,
    pub product_id: Option<u16>,
    pub class_code: Option<u8>,
    pub sub_class_code: Option<u8>,
    pub protocol_code: Option<u8>,
}

impl HotplugDeviceInfo {
    fn from_device_info(device: &DeviceInfo) -> Self {
        Self {
            #[cfg(target_os = "linux")]
            bus_number: device.busnum(),
            #[cfg(not(target_os = "linux"))]
            bus_number: 0,
            address: device.device_address(),
            #[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
            port_numbers: device.port_chain().to_vec(),
            #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
            port_numbers: Vec::new(),
            vendor_id: Some(device.vendor_id()),
            product_id: Some(device.product_id()),
            class_code: Some(device.class()),
            sub_class_code: Some(device.subclass()),
            protocol_code: Some(device.protocol()),
        }
    }
}

#[derive(Debug, Clone)]
pub struct HotplugEvent {
    pub kind: HotplugEventKind,
    pub device: HotplugDeviceInfo,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct HotplugOptions {
    pub vendor_id: Option<u16>,
    pub product_id: Option<u16>,
    pub class_code: Option<u8>,
    pub enumerate: bool,
}

#[derive(Debug, Clone, Default)]
pub struct Probe {
    transport: TransportConfig,
}

impl Probe {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_transport_config(transport: TransportConfig) -> Self {
        Self { transport }
    }

    pub fn transport_config(&self) -> &TransportConfig {
        &self.transport
    }

    pub fn watch<F>(&self, options: HotplugOptions, callback: F) -> Result<HotplugRegistration>
    where
        F: FnMut(HotplugEvent) + Send + 'static,
    {
        UsbDevice::with_transport_config(self.transport)?
            .register_hotplug_callback(options, callback)
    }
}

pub struct UsbDevice {
    handle: Option<Device>,
    interface: Option<Interface>,
    transport: TransportConfig,
}

impl UsbDevice {
    pub fn with_transport_config(transport: TransportConfig) -> Result<Self> {
        Ok(Self {
            handle: None,
            interface: None,
            transport,
        })
    }

    pub fn is_open(&self) -> bool {
        self.interface.is_some()
    }

    pub fn transport_config(&self) -> &TransportConfig {
        &self.transport
    }

    pub fn open(&mut self, vid: u16, pid: u16) -> Result<()> {
        if self.is_open() {
            return Ok(());
        }

        let device_info = nusb::list_devices()
            .wait()
            .map_err(|err| usb_error(err, "nusb_list_devices"))?
            .find(|device| device.vendor_id() == vid && device.product_id() == pid)
            .ok_or(Error::DeviceNotFound { vid, pid })?;

        let device = device_info
            .open()
            .wait()
            .map_err(|err| usb_error(err, "nusb_open_device"))?;

        if self.transport.reset_on_open {
            device
                .reset()
                .wait()
                .map_err(|err| usb_error(err, "nusb_reset_device"))?;
        }

        let interface = device
            .detach_and_claim_interface(INTERFACE)
            .wait()
            .map_err(|err| usb_error(err, "nusb_claim_interface"))?;

        let mut usb_device = Self {
            handle: Some(device),
            interface: Some(interface),
            transport: self.transport,
        };

        if usb_device.transport.clear_halt_on_open {
            usb_device.clear_halt_all()?;
        }

        *self = usb_device;
        Ok(())
    }

    pub fn close(&mut self) -> Result<()> {
        self.interface.take();
        self.handle.take();
        Ok(())
    }

    pub fn read_bytes(&self, endpoint: Endpoint, buffer: &mut [u8]) -> Result<()> {
        let interface = self.interface.as_ref().ok_or(Error::DeviceNotOpen)?;
        bulk_read(interface, endpoint, buffer, self.transport.usb_timeout)
    }

    pub fn read_words(&self, endpoint: Endpoint, buffer: &mut [u16]) -> Result<()> {
        let raw = words_as_bytes_mut(buffer);
        self.read_bytes(endpoint, raw)
    }

    pub fn write_bytes(&self, endpoint: Endpoint, buffer: &[u8]) -> Result<()> {
        let interface = self.interface.as_ref().ok_or(Error::DeviceNotOpen)?;
        bulk_write(interface, endpoint, buffer, self.transport.usb_timeout)
    }

    pub fn write_words(&self, endpoint: Endpoint, buffer: &[u16]) -> Result<()> {
        let raw = words_as_bytes(buffer);
        self.write_bytes(endpoint, raw)
    }

    pub fn open_in_endpoint(&self, endpoint: Endpoint) -> Result<nusb::Endpoint<Bulk, In>> {
        let interface = self.interface.as_ref().ok_or(Error::DeviceNotOpen)?;
        interface
            .endpoint::<Bulk, In>(endpoint as u8)
            .map_err(|err| usb_error(err, "nusb_open_in_endpoint"))
    }

    pub fn open_out_endpoint(&self, endpoint: Endpoint) -> Result<nusb::Endpoint<Bulk, Out>> {
        let interface = self.interface.as_ref().ok_or(Error::DeviceNotOpen)?;
        interface
            .endpoint::<Bulk, Out>(endpoint as u8)
            .map_err(|err| usb_error(err, "nusb_open_out_endpoint"))
    }

    pub fn register_hotplug_callback<F>(
        &self,
        options: HotplugOptions,
        mut callback: F,
    ) -> Result<HotplugRegistration>
    where
        F: FnMut(HotplugEvent) + Send + 'static,
    {
        let mut seen_devices = Vec::<(DeviceId, HotplugDeviceInfo)>::new();
        let initial_devices = matching_devices(options)?;
        if options.enumerate {
            for device in &initial_devices {
                callback(HotplugEvent {
                    kind: HotplugEventKind::Arrived,
                    device: HotplugDeviceInfo::from_device_info(device),
                });
            }
        }
        seen_devices.extend(
            initial_devices
                .iter()
                .map(|device| (device.id(), HotplugDeviceInfo::from_device_info(device))),
        );

        let running = Arc::new(AtomicBool::new(true));
        let thread_running = Arc::clone(&running);
        let thread = thread::Builder::new()
            .name("vlfd-usb-hotplug".into())
            .spawn(move || {
                let mut known = seen_devices;
                while thread_running.load(Ordering::Relaxed) {
                    if let Ok(devices) = matching_devices(options) {
                        let mut current = devices
                            .iter()
                            .map(|device| {
                                (device.id(), HotplugDeviceInfo::from_device_info(device))
                            })
                            .collect::<Vec<_>>();

                        for (id, info) in &current {
                            if !known.iter().any(|(known_id, _)| known_id == id) {
                                callback(HotplugEvent {
                                    kind: HotplugEventKind::Arrived,
                                    device: info.clone(),
                                });
                            }
                        }

                        for (id, info) in &known {
                            if !current.iter().any(|(current_id, _)| current_id == id) {
                                callback(HotplugEvent {
                                    kind: HotplugEventKind::Left,
                                    device: info.clone(),
                                });
                            }
                        }

                        known.clear();
                        known.append(&mut current);
                    }

                    thread::sleep(HOTPLUG_POLL_INTERVAL);
                }
            })
            .map_err(Error::Io)?;

        Ok(HotplugRegistration {
            running,
            thread: Some(thread),
        })
    }

    pub(crate) fn clear_halt_all(&mut self) -> Result<()> {
        for endpoint in [
            Endpoint::FifoWrite,
            Endpoint::Command,
            Endpoint::FifoRead,
            Endpoint::Sync,
        ] {
            self.clear_halt(endpoint)?;
        }
        Ok(())
    }

    fn clear_halt(&mut self, endpoint: Endpoint) -> Result<()> {
        let interface = self.interface.as_ref().ok_or(Error::DeviceNotOpen)?;
        match endpoint {
            Endpoint::FifoWrite | Endpoint::Command => {
                let mut ep = interface
                    .endpoint::<Bulk, Out>(endpoint as u8)
                    .map_err(|err| usb_error(err, "nusb_open_out_endpoint"))?;
                ep.clear_halt()
                    .wait()
                    .map_err(|err| usb_error(err, "nusb_clear_halt"))?;
            }
            Endpoint::FifoRead | Endpoint::Sync => {
                let mut ep = interface
                    .endpoint::<Bulk, In>(endpoint as u8)
                    .map_err(|err| usb_error(err, "nusb_open_in_endpoint"))?;
                ep.clear_halt()
                    .wait()
                    .map_err(|err| usb_error(err, "nusb_clear_halt"))?;
            }
        }
        Ok(())
    }
}

impl Drop for UsbDevice {
    fn drop(&mut self) {
        let _ = self.close();
    }
}

#[derive(Debug)]
pub struct HotplugRegistration {
    running: Arc<AtomicBool>,
    thread: Option<thread::JoinHandle<()>>,
}

impl Drop for HotplugRegistration {
    fn drop(&mut self) {
        self.running.store(false, Ordering::SeqCst);
        if let Some(handle) = self.thread.take() {
            let _ = handle.join();
        }
    }
}

fn bulk_read(
    interface: &Interface,
    endpoint: Endpoint,
    buffer: &mut [u8],
    timeout: Duration,
) -> Result<()> {
    let mut reader = interface
        .endpoint::<Bulk, In>(endpoint as u8)
        .map_err(|err| usb_error(err, "nusb_open_in_endpoint"))?
        .reader(IO_BUFFER_SIZE)
        .with_read_timeout(timeout);

    reader
        .read_exact(buffer)
        .map_err(|err| io_error(err, "nusb_bulk_read"))?;
    Ok(())
}

fn bulk_write(
    interface: &Interface,
    endpoint: Endpoint,
    buffer: &[u8],
    timeout: Duration,
) -> Result<()> {
    let mut writer = interface
        .endpoint::<Bulk, Out>(endpoint as u8)
        .map_err(|err| usb_error(err, "nusb_open_out_endpoint"))?
        .writer(IO_BUFFER_SIZE)
        .with_write_timeout(timeout);

    writer
        .write_all(buffer)
        .map_err(|err| io_error(err, "nusb_bulk_write"))?;
    writer
        .flush()
        .map_err(|err| io_error(err, "nusb_bulk_flush"))?;
    Ok(())
}

fn matching_devices(options: HotplugOptions) -> Result<Vec<DeviceInfo>> {
    let devices = nusb::list_devices()
        .wait()
        .map_err(|err| usb_error(err, "nusb_list_devices"))?;
    Ok(devices
        .filter(|device| {
            options
                .vendor_id
                .is_none_or(|vendor_id| device.vendor_id() == vendor_id)
                && options
                    .product_id
                    .is_none_or(|product_id| device.product_id() == product_id)
                && options
                    .class_code
                    .is_none_or(|class_code| device.class() == class_code)
        })
        .collect())
}

fn words_as_bytes(words: &[u16]) -> &[u8] {
    unsafe { std::slice::from_raw_parts(words.as_ptr() as *const u8, std::mem::size_of_val(words)) }
}

fn words_as_bytes_mut(words: &mut [u16]) -> &mut [u8] {
    unsafe {
        std::slice::from_raw_parts_mut(words.as_mut_ptr() as *mut u8, std::mem::size_of_val(words))
    }
}

fn usb_error(err: nusb::Error, context: &'static str) -> Error {
    Error::Usb {
        source: Box::new(err),
        context,
    }
}

fn io_error(err: std::io::Error, context: &'static str) -> Error {
    if err.kind() == std::io::ErrorKind::TimedOut {
        Error::Timeout(context)
    } else {
        Error::Usb {
            source: Box::new(err),
            context,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::TransportConfig;
    use std::time::Duration;

    #[test]
    fn default_transport_config_prefers_stable_open_behavior() {
        let config = TransportConfig::default();
        assert_eq!(config.usb_timeout, Duration::from_millis(1_000));
        assert_eq!(config.sync_timeout, Duration::from_secs(1));
        assert!(!config.reset_on_open);
        assert!(config.clear_halt_on_open);
    }
}
