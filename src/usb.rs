use crate::error::{Error, Result};
use rusb::{
    self, Context, Device, DeviceHandle, Hotplug, HotplugBuilder, Registration, UsbContext,
};
use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::Duration,
};

const INTERFACE: u8 = 0;
const DEFAULT_TIMEOUT: Duration = Duration::from_millis(1_000);

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
    fn from_device(device: &Device<Context>) -> Self {
        let descriptor = device.device_descriptor().ok();
        Self {
            bus_number: device.bus_number(),
            address: device.address(),
            port_numbers: device.port_numbers().unwrap_or_default(),
            vendor_id: descriptor.as_ref().map(|desc| desc.vendor_id()),
            product_id: descriptor.as_ref().map(|desc| desc.product_id()),
            class_code: descriptor.as_ref().map(|desc| desc.class_code()),
            sub_class_code: descriptor.as_ref().map(|desc| desc.sub_class_code()),
            protocol_code: descriptor.as_ref().map(|desc| desc.protocol_code()),
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

/// Thin wrapper around a `rusb` device handle that offers higher level helpers
/// for bulk transfers and automatic cleanup.
pub struct UsbDevice {
    context: Context,
    handle: Option<DeviceHandle<Context>>,
}

impl UsbDevice {
    pub fn new() -> Result<Self> {
        let context = Context::new().map_err(|err| usb_error(err, "libusb_init"))?;
        Ok(Self {
            context,
            handle: None,
        })
    }

    pub fn is_open(&self) -> bool {
        self.handle.is_some()
    }

    pub fn open(&mut self, vid: u16, pid: u16) -> Result<()> {
        if self.is_open() {
            return Ok(());
        }

        let handle = self
            .context
            .open_device_with_vid_pid(vid, pid)
            .ok_or(Error::DeviceNotFound { vid, pid })?;

        handle
            .reset()
            .map_err(|err| usb_error(err, "libusb_reset_device"))?;
        handle
            .claim_interface(INTERFACE)
            .map_err(|err| usb_error(err, "libusb_claim_interface"))?;

        for endpoint in [
            Endpoint::FifoWrite,
            Endpoint::Command,
            Endpoint::FifoRead,
            Endpoint::Sync,
        ] {
            handle
                .clear_halt(endpoint as u8)
                .map_err(|err| usb_error(err, "libusb_clear_halt"))?;
        }

        self.handle = Some(handle);
        Ok(())
    }

    pub fn close(&mut self) -> Result<()> {
        if let Some(handle) = self.handle.take() {
            match handle.release_interface(INTERFACE) {
                Ok(_) | Err(rusb::Error::NoDevice) => {}
                Err(err) => return Err(usb_error(err, "libusb_release_interface")),
            }
        }
        Ok(())
    }

    pub fn read_bytes(&self, endpoint: Endpoint, buffer: &mut [u8]) -> Result<()> {
        let handle = self.handle.as_ref().ok_or(Error::DeviceNotOpen)?;
        bulk_read(handle, endpoint, buffer)
    }

    pub fn read_words(&self, endpoint: Endpoint, buffer: &mut [u16]) -> Result<()> {
        let raw = words_as_bytes_mut(buffer);
        self.read_bytes(endpoint, raw)
    }

    pub fn write_bytes(&self, endpoint: Endpoint, buffer: &[u8]) -> Result<()> {
        let handle = self.handle.as_ref().ok_or(Error::DeviceNotOpen)?;
        bulk_write(handle, endpoint, buffer)
    }

    pub fn write_words(&self, endpoint: Endpoint, buffer: &[u16]) -> Result<()> {
        let raw = words_as_bytes(buffer);
        self.write_bytes(endpoint, raw)
    }

    pub fn register_hotplug_callback<F>(
        &self,
        options: HotplugOptions,
        callback: F,
    ) -> Result<HotplugRegistration>
    where
        F: FnMut(HotplugEvent) + Send + 'static,
    {
        if !rusb::has_hotplug() {
            return Err(Error::FeatureUnavailable("usb_hotplug"));
        }

        let mut builder = HotplugBuilder::new();
        if let Some(vendor) = options.vendor_id {
            builder.vendor_id(vendor);
        }
        if let Some(product) = options.product_id {
            builder.product_id(product);
        }
        if let Some(class_code) = options.class_code {
            builder.class(class_code);
        }
        builder.enumerate(options.enumerate);

        let handler = CallbackHotplug { callback };

        let registration = builder
            .register(&self.context, Box::new(handler))
            .map_err(|err| usb_error(err, "libusb_hotplug_register_callback"))?;

        HotplugRegistration::new(self.context.clone(), registration)
    }
}

impl Drop for UsbDevice {
    fn drop(&mut self) {
        let _ = self.close();
    }
}

#[derive(Debug)]
pub struct HotplugRegistration {
    registration: Option<Registration<Context>>,
    running: Arc<AtomicBool>,
    thread: Option<thread::JoinHandle<()>>,
}

impl HotplugRegistration {
    fn new(context: Context, registration: Registration<Context>) -> Result<Self> {
        let running = Arc::new(AtomicBool::new(true));
        let thread_running = Arc::clone(&running);

        let thread = thread::Builder::new()
            .name("vlfd-usb-hotplug".into())
            .spawn(move || {
                while thread_running.load(Ordering::Relaxed) {
                    match context.handle_events(Some(Duration::from_millis(100))) {
                        Ok(_) => {}
                        Err(rusb::Error::Interrupted) | Err(rusb::Error::Timeout) => continue,
                        Err(_) => break,
                    }
                }
            })
            .map_err(Error::Io)?;

        Ok(Self {
            registration: Some(registration),
            running,
            thread: Some(thread),
        })
    }
}

impl Drop for HotplugRegistration {
    fn drop(&mut self) {
        let _ = self.registration.take();
        self.running.store(false, Ordering::SeqCst);
        if let Some(handle) = self.thread.take() {
            let _ = handle.join();
        }
    }
}

struct CallbackHotplug<F>
where
    F: FnMut(HotplugEvent) + Send + 'static,
{
    callback: F,
}

impl<F> Hotplug<Context> for CallbackHotplug<F>
where
    F: FnMut(HotplugEvent) + Send + 'static,
{
    fn device_arrived(&mut self, device: Device<Context>) {
        (self.callback)(HotplugEvent {
            kind: HotplugEventKind::Arrived,
            device: HotplugDeviceInfo::from_device(&device),
        });
    }

    fn device_left(&mut self, device: Device<Context>) {
        (self.callback)(HotplugEvent {
            kind: HotplugEventKind::Left,
            device: HotplugDeviceInfo::from_device(&device),
        });
    }
}

fn bulk_read<T: UsbContext>(
    handle: &DeviceHandle<T>,
    endpoint: Endpoint,
    buffer: &mut [u8],
) -> Result<()> {
    let mut offset = 0;
    while offset < buffer.len() {
        let chunk = &mut buffer[offset..];
        let bytes_read = handle
            .read_bulk(endpoint as u8, chunk, DEFAULT_TIMEOUT)
            .map_err(|err| usb_error(err, "libusb_bulk_transfer"))?;

        if bytes_read == 0 {
            return Err(Error::UnexpectedResponse("bulk read returned zero bytes"));
        }

        offset += bytes_read;
    }
    Ok(())
}

fn bulk_write<T: UsbContext>(
    handle: &DeviceHandle<T>,
    endpoint: Endpoint,
    buffer: &[u8],
) -> Result<()> {
    let mut offset = 0;
    while offset < buffer.len() {
        let chunk = &buffer[offset..];
        let bytes_written = handle
            .write_bulk(endpoint as u8, chunk, DEFAULT_TIMEOUT)
            .map_err(|err| usb_error(err, "libusb_bulk_transfer"))?;

        if bytes_written == 0 {
            return Err(Error::UnexpectedResponse("bulk write returned zero bytes"));
        }

        offset += bytes_written;
    }
    Ok(())
}

fn words_as_bytes(words: &[u16]) -> &[u8] {
    unsafe { std::slice::from_raw_parts(words.as_ptr() as *const u8, std::mem::size_of_val(words)) }
}

fn words_as_bytes_mut(words: &mut [u16]) -> &mut [u8] {
    unsafe {
        std::slice::from_raw_parts_mut(words.as_mut_ptr() as *mut u8, std::mem::size_of_val(words))
    }
}

fn usb_error(err: rusb::Error, context: &'static str) -> Error {
    Error::Usb {
        source: err,
        context,
    }
}
