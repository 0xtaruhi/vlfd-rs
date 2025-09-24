use crate::error::{Error, Result};
use rusb::{Context, DeviceHandle, UsbContext};
use std::time::Duration;

const INTERFACE: u8 = 0;
const DEFAULT_TIMEOUT: Duration = Duration::from_millis(1_000);

#[derive(Debug, Clone, Copy)]
pub enum Endpoint {
    FifoWrite = 0x02,
    Command = 0x04,
    FifoRead = 0x86,
    Sync = 0x88,
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
}

impl Drop for UsbDevice {
    fn drop(&mut self) {
        let _ = self.close();
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
