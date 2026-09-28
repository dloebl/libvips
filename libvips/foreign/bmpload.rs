//! load bmp from a file or a buffer
//!
//! Only the most common BMP variant: BITMAPINFOHEADER, 24-bit, uncompressed,
//! bottom-up. Anything else is rejected with an error. The data is read
//! once, then pixels are generated on demand straight from it.

use std::ffi::{c_char, CStr};
use std::io::Read;
use std::path::PathBuf;
use std::sync::Arc;

use vips::foreign::{IsABuffer, IsAFile, Load};
use vips::sys;
use vips::{field, object_type};
use vips::{Arg, Class, Error, Generate, Image, ObjectSubclass, OutRegion, Result};

/// bmpload
#[repr(C)]
pub struct BmpLoadFile {
    parent: sys::VipsForeignLoad,
    /// the "filename" argument, owned by vips
    filename: *mut c_char,
    /// parsed in header(), shared with the generate callback
    bmp: Option<Arc<Bmp>>,
}

/// bmpload_buffer
#[repr(C)]
pub struct BmpLoadBuffer {
    parent: sys::VipsForeignLoad,
    /// the "buffer" argument, owned by vips
    buffer: *mut sys::VipsBlob,
    bmp: Option<Arc<Bmp>>,
}

/// A parsed file.
#[derive(Debug)]
struct Bmp {
    width: usize,
    height: usize,
    /// bytes per line, padded to 4
    stride: usize,
    data_offset: usize,
    bytes: Vec<u8>,
}

const HEADER_SIZE: usize = 54;

impl Bmp {
    fn parse(bytes: Vec<u8>) -> Result<Bmp> {
        if bytes.len() < HEADER_SIZE || &bytes[..2] != b"BM" {
            return Err(Error::new("not a bmp file"));
        }
        let le16 = |o: usize| u16::from_le_bytes([bytes[o], bytes[o + 1]]) as usize;
        let le32 = |o: usize| u32::from_le_bytes(bytes[o..o + 4].try_into().unwrap());

        // BITMAPINFOHEADER, 24 bits per pixel, BI_RGB
        if le32(14) != 40 || le16(28) != 24 || le32(30) != 0 {
            return Err(Error::new(
                "unsupported bmp variant, only 24-bit uncompressed is supported",
            ));
        }

        // signed, and negative means top-down, which we don't support
        let width = le32(18) as i32;
        let height = le32(22) as i32;
        if !(1..=65535).contains(&width) || !(1..=65535).contains(&height) {
            return Err(Error::new("bad image dimensions"));
        }
        let (width, height) = (width as usize, height as usize);

        let stride = (3 * width).next_multiple_of(4);
        let data_offset = le32(10) as usize;
        let data_end = stride
            .checked_mul(height)
            .and_then(|size| size.checked_add(data_offset));
        if data_offset < HEADER_SIZE || !data_end.is_some_and(|end| end <= bytes.len()) {
            return Err(Error::new("truncated pixel data"));
        }

        Ok(Bmp {
            width,
            height,
            stride,
            data_offset,
            bytes,
        })
    }

    /// Line `y`, counting from the top, as BGR.
    fn line(&self, y: usize) -> &[u8] {
        // lines are stored bottom-up
        let start = self.data_offset + (self.height - 1 - y) * self.stride;
        &self.bytes[start..start + 3 * self.width]
    }

    fn init(&self, image: &Image) -> Result<()> {
        image.init_fields(
            self.width as i32,
            self.height as i32,
            3,
            sys::VIPS_FORMAT_UCHAR,
            sys::VIPS_CODING_NONE,
            sys::VIPS_INTERPRETATION_sRGB,
            1.0,
            1.0,
        );
        image.pipeline(sys::VIPS_DEMAND_STYLE_ANY, &[])
    }
}

/// Generates pixels from the file data, on any thread.
struct Pixels(Arc<Bmp>);

impl Generate for Pixels {
    type Seq = ();
    const DOMAIN: &'static CStr = c"bmpload";

    fn start(&self) -> Result<()> {
        Ok(())
    }

    fn generate(&self, _: &mut (), out: &mut OutRegion<'_>) -> Result<()> {
        let left = out.valid().left as usize;

        out.for_each_line(|y, line: &mut [u8]| {
            let bgr = &self.0.line(y as usize)[3 * left..];
            for (rgb, bgr) in line.chunks_exact_mut(3).zip(bgr.chunks_exact(3)) {
                rgb.copy_from_slice(&[bgr[2], bgr[1], bgr[0]]);
            }
        });

        Ok(())
    }
}

/// What the file and buffer loaders share.
fn header(slot: &mut Option<Arc<Bmp>>, bytes: Vec<u8>, out: &Image) -> Result<()> {
    let bmp = Arc::new(Bmp::parse(bytes)?);
    bmp.init(out)?;
    *slot = Some(bmp);

    Ok(())
}

fn load(slot: &Option<Arc<Bmp>>, real: &Image) -> Result<()> {
    let bmp = slot.clone().ok_or_else(|| Error::new("no header"))?;
    bmp.init(real)?;

    real.generate(Pixels(bmp))
}

fn is_a(header: &[u8]) -> bool {
    header.len() >= HEADER_SIZE && &header[..2] == b"BM"
}

fn class_init<T: Load>(class: &mut Class<T>) {
    class.install_load();
    class.set_suffixes(&[c".bmp", c".dib"]);
    // not ready to be the default BMP loader, keep preferring
    // magickload (priority -100)
    class.set_priority(-200);
}

// SAFETY: BmpLoadFile is repr(C), starts with its parent, and is valid
// zeroed. The Arc is dropped in finalize().
unsafe impl ObjectSubclass for BmpLoadFile {
    const NAME: &'static CStr = c"VipsForeignLoadBmpFile";
    const NICKNAME: &'static CStr = c"bmpload";
    const DESCRIPTION: &'static CStr = c"load bmp from file";
    type Class = sys::VipsForeignLoadClass;

    fn parent_type() -> sys::GType {
        // SAFETY: plain type lookup
        unsafe { sys::vips_foreign_load_get_type() }
    }

    fn class_init(class: &mut Class<Self>) {
        class_init(class);
        class.install_is_a();
        class.arg_string(
            Arg::required_input(c"filename", 1, c"Filename", c"Filename to load from"),
            field!(BmpLoadFile, filename),
            None,
        );
    }

    fn finalize(&mut self) {
        self.bmp = None;
    }
}

impl Load for BmpLoadFile {
    fn header(&mut self, out: &Image) -> Result<()> {
        // SAFETY: vips keeps the argument alive while we run
        let path = unsafe { vips::foreign::path(self.filename) }
            .ok_or_else(|| Error::new("no filename set"))?;
        let bytes = std::fs::read(&path)
            .map_err(|e| Error::new(format!("unable to read \"{}\": {e}", path.display())))?;

        header(&mut self.bmp, bytes, out)
    }

    fn load(&mut self, real: &Image) -> Result<()> {
        load(&self.bmp, real)
    }

    fn flags(&self) -> sys::VipsForeignFlags {
        // any area is a copy from memory
        sys::VIPS_FOREIGN_PARTIAL
    }
}

impl IsAFile for BmpLoadFile {
    fn is_a(path: PathBuf) -> bool {
        let mut header = [0u8; HEADER_SIZE];
        std::fs::File::open(path)
            .and_then(|mut file| file.read_exact(&mut header))
            .is_ok_and(|()| is_a(&header))
    }
}

// SAFETY: as for BmpLoadFile
unsafe impl ObjectSubclass for BmpLoadBuffer {
    const NAME: &'static CStr = c"VipsForeignLoadBmpBuffer";
    const NICKNAME: &'static CStr = c"bmpload_buffer";
    const DESCRIPTION: &'static CStr = c"load bmp from buffer";
    type Class = sys::VipsForeignLoadClass;

    fn parent_type() -> sys::GType {
        // SAFETY: plain type lookup
        unsafe { sys::vips_foreign_load_get_type() }
    }

    fn class_init(class: &mut Class<Self>) {
        class_init(class);
        class.install_is_a_buffer();
        class.arg_boxed(
            Arg::required_input(c"buffer", 1, c"Buffer", c"Buffer to load from"),
            field!(BmpLoadBuffer, buffer),
            // SAFETY: plain type lookup
            unsafe { sys::vips_blob_get_type() },
        );
    }

    fn finalize(&mut self) {
        self.bmp = None;
    }
}

impl Load for BmpLoadBuffer {
    fn header(&mut self, out: &Image) -> Result<()> {
        // SAFETY: vips keeps the argument alive while we run
        let bytes = unsafe { vips::foreign::blob(self.buffer) }
            .ok_or_else(|| Error::new("no buffer set"))?;

        header(&mut self.bmp, bytes.to_vec(), out)
    }

    fn load(&mut self, real: &Image) -> Result<()> {
        load(&self.bmp, real)
    }

    fn flags(&self) -> sys::VipsForeignFlags {
        sys::VIPS_FOREIGN_PARTIAL
    }
}

impl IsABuffer for BmpLoadBuffer {
    fn is_a_buffer(data: &[u8]) -> bool {
        is_a(data)
    }
}

// Called from vips_foreign_operation_init() in foreign.c.
object_type!(vips_foreign_load_bmp_file_get_type, BmpLoadFile);
object_type!(vips_foreign_load_bmp_buffer_get_type, BmpLoadBuffer);

#[cfg(test)]
mod tests {
    use super::*;

    /// A 24-bit bottom-up bmp with a one-pixel-per-line pattern.
    fn bmp(width: u32, height: u32) -> Vec<u8> {
        let stride = (3 * width).next_multiple_of(4);
        let mut bytes = vec![0u8; HEADER_SIZE];
        bytes[..2].copy_from_slice(b"BM");
        bytes[10..14].copy_from_slice(&(HEADER_SIZE as u32).to_le_bytes());
        bytes[14..18].copy_from_slice(&40u32.to_le_bytes());
        bytes[18..22].copy_from_slice(&width.to_le_bytes());
        bytes[22..26].copy_from_slice(&height.to_le_bytes());
        bytes[26..28].copy_from_slice(&1u16.to_le_bytes());
        bytes[28..30].copy_from_slice(&24u16.to_le_bytes());
        for y in 0..height {
            let mut line = vec![0u8; stride as usize];
            // blue = the line number, bottom-up
            line[0] = y as u8;
            bytes.extend(line);
        }
        bytes
    }

    #[test]
    fn parses() {
        let bmp = Bmp::parse(bmp(3, 5)).unwrap();
        assert_eq!((bmp.width, bmp.height, bmp.stride), (3, 5, 12));
        // top line is the last one stored
        assert_eq!(bmp.line(0)[0], 4);
        assert_eq!(bmp.line(4)[0], 0);
    }

    #[test]
    fn rejects_bad_files() {
        assert!(Bmp::parse(vec![]).is_err());
        assert!(Bmp::parse(b"BM".to_vec()).is_err());

        let mut truncated = bmp(3, 5);
        truncated.pop();
        assert!(Bmp::parse(truncated).is_err());

        let mut top_down = bmp(3, 5);
        top_down[22..26].copy_from_slice(&(-5i32).to_le_bytes());
        assert!(Bmp::parse(top_down).is_err());

        let mut paletted = bmp(3, 5);
        paletted[28..30].copy_from_slice(&8u16.to_le_bytes());
        assert!(Bmp::parse(paletted).is_err());

        let mut huge_offset = bmp(3, 5);
        huge_offset[10..14].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(Bmp::parse(huge_offset).is_err());
    }
}
