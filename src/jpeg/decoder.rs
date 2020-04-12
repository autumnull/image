use std::convert::TryFrom;
use std::io::{self, Cursor, Read};
use std::marker::PhantomData;
use std::mem;

use crate::color::ColorType;
use crate::error::{
    DecodingError, ImageError, ImageResult, UnsupportedError, UnsupportedErrorKind,
};
use crate::image::{ImageDecoder, ImageFormat};

/// JPEG decoder
pub struct JpegDecoder<R> {
    decoder: jpeg::Decoder<R>,
    metadata: jpeg::ImageInfo,
}

impl<R: Read> JpegDecoder<R> {
    /// Create a new decoder that decodes from the stream ```r```
    pub fn new(r: R) -> ImageResult<JpegDecoder<R>> {
        let mut decoder = jpeg::Decoder::new(r);

        decoder.read_info().map_err(ImageError::from_jpeg)?;
        let mut metadata = decoder.info().unwrap();

        // We convert CMYK data to RGB before returning it to the user.
        if metadata.pixel_format == jpeg::PixelFormat::CMYK32 {
            metadata.pixel_format = jpeg::PixelFormat::RGB24;
        }

        Ok(JpegDecoder {
            decoder,
            metadata,
        })
    }
}

/// Wrapper struct around a `Cursor<Vec<u8>>`
pub struct JpegReader<R>(Cursor<Vec<u8>>, PhantomData<R>);
impl<R> Read for JpegReader<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.0.read(buf)
    }
    fn read_to_end(&mut self, buf: &mut Vec<u8>) -> io::Result<usize> {
        if self.0.position() == 0 && buf.is_empty() {
            mem::swap(buf, self.0.get_mut());
            Ok(buf.len())
        } else {
            self.0.read_to_end(buf)
        }
    }
}

impl<'a, R: 'a + Read> ImageDecoder<'a> for JpegDecoder<R> {
    type Reader = JpegReader<R>;

    fn dimensions(&self) -> (u32, u32) {
        (u32::from(self.metadata.width), u32::from(self.metadata.height))
    }

    fn color_type(&self) -> ColorType {
        ColorType::from_jpeg(self.metadata.pixel_format)
    }

    fn into_reader(mut self) -> ImageResult<Self::Reader> {
        let mut data = self.decoder.decode().map_err(ImageError::from_jpeg)?;
        data = match self.decoder.info().unwrap().pixel_format {
            jpeg::PixelFormat::CMYK32 => cmyk_to_rgb(&data),
            _ => data,
        };

        Ok(JpegReader(Cursor::new(data), PhantomData))
    }

    fn read_image(mut self, buf: &mut [u8]) -> ImageResult<()> {
        assert_eq!(u64::try_from(buf.len()), Ok(self.total_bytes()));

        let mut data = self.decoder.decode().map_err(ImageError::from_jpeg)?;
        data = match self.decoder.info().unwrap().pixel_format {
            jpeg::PixelFormat::CMYK32 => cmyk_to_rgb(&data),
            _ => data,
        };

        buf.copy_from_slice(&data);
        Ok(())
    }
}

fn cmyk_to_rgb(input: &[u8]) -> Vec<u8> {
    let count = input.len() / 4;
    let mut output = vec![0; 3 * count];

    let in_pixels = input[..4 * count].chunks_exact(4);
    let out_pixels = output[..3 * count].chunks_exact_mut(3);

    for (pixel, outp) in in_pixels.zip(out_pixels) {
        let c = 255 - u16::from(pixel[0]);
        let m = 255 - u16::from(pixel[1]);
        let y = 255 - u16::from(pixel[2]);
        let k = 255 - u16::from(pixel[3]);
        // CMY -> RGB
        let r = (k * c) / 255;
        let g = (k * m) / 255;
        let b = (k * y) / 255;

        outp[0] = r as u8;
        outp[1] = g as u8;
        outp[2] = b as u8;
    }

    output
}

impl ColorType {
    fn from_jpeg(pixel_format: jpeg::PixelFormat) -> ColorType {
        use jpeg::PixelFormat::*;
        match pixel_format {
            L8 => ColorType::L8,
            RGB24 => ColorType::Rgb8,
            CMYK32 => panic!(),
        }
    }
}

impl ImageError {
    fn from_jpeg(err: jpeg::Error) -> ImageError {
        use jpeg::Error::*;
        match err {
            err @ Format(_) => {
                ImageError::Decoding(DecodingError::new(ImageFormat::Jpeg.into(), err))
            }
            Unsupported(desc) => ImageError::Unsupported(UnsupportedError::from_format_and_kind(
                ImageFormat::Jpeg.into(),
                UnsupportedErrorKind::GenericFeature(format!("{:?}", desc)),
            )),
            Io(err) => ImageError::IoError(err),
            Internal(err) => {
                ImageError::Decoding(DecodingError::new(ImageFormat::Jpeg.into(), err))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    #[cfg(feature = "benchmarks")]
    extern crate test;

    use super::cmyk_to_rgb;
    #[cfg(feature = "benchmarks")]
    use test::Bencher;

    const W: usize = 256;
    const H: usize = 256;

    #[test]
    fn cmyk_to_rgb_correct() {
        for c in 0..=255 {
            for k in 0..=255 {
                // Based on R = 255 * (1-C/255) * (1-K/255)
                let r = (255.0 - f32::from(c)) * (255.0 - f32::from(k)) / 255.0;
                let r_u8 = r as u8;
                let convert_r = cmyk_to_rgb(&[c, 0, 0, k])[0];
                let convert_g = cmyk_to_rgb(&[0, c, 0, k])[1];
                let convert_b = cmyk_to_rgb(&[0, 0, c, k])[2];

                assert_eq!(convert_r, r_u8,
                           "c = {}, k = {}, cymk_to_rgb[0] = {}, should be {}", c, k, convert_r, r_u8);
                assert_eq!(convert_g, r_u8,
                           "m = {}, k = {}, cymk_to_rgb[1] = {}, should be {}", c, k, convert_g, r_u8);
                assert_eq!(convert_b, r_u8,
                           "y = {}, k = {}, cymk_to_rgb[2] = {}, should be {}", c, k, convert_b, r_u8);

            }
        }
    }

    fn single_pix_correct(cmyk_pix: [u8; 4], rgb_pix_true: [u8; 3]) {
        let rgb_pix = cmyk_to_rgb(&cmyk_pix);
        assert_eq!(rgb_pix_true[0], rgb_pix[0], "With CMYK {:?} expected {:?}, got {:?}", cmyk_pix, rgb_pix_true, rgb_pix);
        assert_eq!(rgb_pix_true[1], rgb_pix[1], "With CMYK {:?} expected {:?}, got {:?}", cmyk_pix, rgb_pix_true, rgb_pix);
        assert_eq!(rgb_pix_true[2], rgb_pix[2], "With CMYK {:?} expected {:?}, got {:?}", cmyk_pix, rgb_pix_true, rgb_pix);
    }

    #[test]
    fn test_assorted_colors() {
        let cmyk_pixels = vec![[0, 51, 102, 65], [153, 204, 0, 65], [0, 0, 0, 67], [0, 85, 170, 69], [0, 0, 0, 71], [0, 0, 0, 73], [0, 17, 34, 75], [51, 68, 85, 75], [102, 119, 136, 75], [153, 170, 187, 75], [204, 221, 238, 75], [0, 0, 0, 77], [0, 0, 0, 79], [0, 85, 170, 81], [0, 0, 0, 83], [0, 3, 6, 85], [9, 12, 15, 85], [18, 21, 24, 85], [27, 30, 33, 85], [36, 39, 42, 85], [45, 48, 51, 85], [54, 57, 60, 85], [63, 66, 69, 85], [72, 75, 78, 85], [81, 84, 87, 85], [90, 93, 96, 85], [99, 102, 105, 85], [108, 111, 114, 85], [117, 120, 123, 85], [126, 129, 132, 85], [135, 138, 141, 85], [144, 147, 150, 85], [153, 156, 159, 85], [162, 165, 168, 85], [171, 174, 177, 85], [180, 183, 186, 85], [189, 192, 195, 85], [198, 201, 204, 85], [207, 210, 213, 85], [216, 219, 222, 85], [225, 228, 231, 85], [234, 237, 240, 85], [243, 246, 249, 85], [252, 0, 0, 85], [0, 85, 170, 87], [0, 0, 0, 89], [0, 0, 0, 91], [0, 85, 170, 93], [0, 51, 102, 95], [153, 204, 0, 95], [0, 0, 0, 97], [0, 85, 170, 99], [0, 0, 0, 101], [0, 0, 0, 103], [0, 17, 34, 105], [51, 68, 85, 105], [102, 119, 136, 105], [153, 170, 187, 105], [204, 221, 238, 105], [0, 0, 0, 107], [0, 0, 0, 109], [0, 85, 170, 111], [0, 0, 0, 113], [0, 51, 102, 115], [153, 204, 0, 115], [0, 85, 170, 117], [0, 15, 30, 119], [45, 60, 75, 119], [90, 105, 120, 119], [135, 150, 165, 119], [180, 195, 210, 119], [225, 240, 0, 119], [0, 0, 0, 121], [0, 85, 170, 123], [0, 51, 102, 125], [153, 204, 0, 125], [0, 0, 0, 127], [0, 0, 0, 128], [0, 85, 170, 129], [0, 51, 102, 130], [153, 204, 0, 130], [0, 0, 0, 131], [0, 85, 170, 132], [0, 0, 0, 133], [0, 0, 0, 134], [0, 17, 34, 135], [51, 68, 85, 135], [102, 119, 136, 135], [153, 170, 187, 135], [204, 221, 238, 135], [0, 15, 30, 136], [45, 60, 75, 136], [90, 105, 120, 136], [135, 150, 165, 136], [180, 195, 210, 136], [225, 240, 0, 136], [0, 0, 0, 137], [0, 85, 170, 138], [0, 0, 0, 139], [0, 51, 102, 140], [153, 204, 0, 140], [0, 85, 170, 141], [0, 0, 0, 142], [0, 0, 0, 143], [0, 85, 170, 144], [0, 51, 102, 145], [153, 204, 0, 145], [0, 0, 0, 146], [0, 85, 170, 147], [0, 0, 0, 148], [0, 0, 0, 149], [0, 17, 34, 150], [51, 68, 85, 150], [102, 119, 136, 150], [153, 170, 187, 150], [204, 221, 238, 150], [0, 0, 0, 151], [0, 0, 0, 152], [0, 5, 10, 153], [15, 20, 25, 153], [30, 35, 40, 153], [45, 50, 55, 153], [60, 65, 70, 153], [75, 80, 85, 153], [90, 95, 100, 153], [105, 110, 115, 153], [120, 125, 130, 153], [135, 140, 145, 153], [150, 155, 160, 153], [165, 170, 175, 153], [180, 185, 190, 153], [195, 200, 205, 153], [210, 215, 220, 153], [225, 230, 235, 153], [240, 245, 250, 153], [0, 0, 0, 154], [0, 51, 102, 155], [153, 204, 0, 155], [0, 85, 170, 156], [0, 0, 0, 157], [0, 0, 0, 158], [0, 85, 170, 159], [0, 51, 102, 160], [153, 204, 0, 160], [0, 0, 0, 161], [0, 85, 170, 162], [0, 0, 0, 163], [0, 0, 0, 164], [0, 17, 34, 165], [51, 68, 85, 165], [102, 119, 136, 165], [153, 170, 187, 165], [204, 221, 238, 165], [0, 0, 0, 166], [0, 0, 0, 167], [0, 85, 170, 168], [0, 0, 0, 169], [0, 3, 6, 170], [9, 12, 15, 170], [18, 21, 24, 170], [27, 30, 33, 170], [36, 39, 42, 170], [45, 48, 51, 170], [54, 57, 60, 170], [63, 66, 69, 170], [72, 75, 78, 170], [81, 84, 87, 170], [90, 93, 96, 170], [99, 102, 105, 170], [108, 111, 114, 170], [117, 120, 123, 170], [126, 129, 132, 170], [135, 138, 141, 170], [144, 147, 150, 170], [153, 156, 159, 170], [162, 165, 168, 170], [171, 174, 177, 170], [180, 183, 186, 170], [189, 192, 195, 170], [198, 201, 204, 170], [207, 210, 213, 170], [216, 219, 222, 170], [225, 228, 231, 170], [234, 237, 240, 170], [243, 246, 249, 170], [252, 0, 0, 170], [0, 85, 170, 171], [0, 0, 0, 172], [0, 0, 0, 173], [0, 85, 170, 174], [0, 51, 102, 175], [153, 204, 0, 175], [0, 0, 0, 176], [0, 85, 170, 177], [0, 0, 0, 178], [0, 0, 0, 179], [0, 17, 34, 180], [51, 68, 85, 180], [102, 119, 136, 180], [153, 170, 187, 180], [204, 221, 238, 180], [0, 0, 0, 181], [0, 0, 0, 182], [0, 85, 170, 183], [0, 0, 0, 184], [0, 51, 102, 185], [153, 204, 0, 185], [0, 85, 170, 186], [0, 15, 30, 187], [45, 60, 75, 187], [90, 105, 120, 187], [135, 150, 165, 187], [180, 195, 210, 187], [225, 240, 0, 187], [0, 0, 0, 188], [0, 85, 170, 189], [0, 51, 102, 190], [153, 204, 0, 190], [0, 0, 0, 191], [0, 85, 170, 192], [0, 0, 0, 193], [0, 0, 0, 194], [0, 17, 34, 195], [51, 68, 85, 195], [102, 119, 136, 195], [153, 170, 187, 195], [204, 221, 238, 195], [0, 0, 0, 196], [0, 0, 0, 197], [0, 85, 170, 198], [0, 0, 0, 199], [0, 51, 102, 200], [153, 204, 0, 200], [0, 85, 170, 201], [0, 0, 0, 202], [0, 0, 0, 203], [0, 5, 10, 204], [15, 20, 25, 204], [30, 35, 40, 204], [45, 50, 55, 204], [60, 65, 70, 204], [75, 80, 85, 204], [90, 95, 100, 204], [105, 110, 115, 204], [120, 125, 130, 204], [135, 140, 145, 204], [150, 155, 160, 204], [165, 170, 175, 204], [180, 185, 190, 204], [195, 200, 205, 204], [210, 215, 220, 204], [225, 230, 235, 204], [240, 245, 250, 204], [0, 51, 102, 205], [153, 204, 0, 205], [0, 0, 0, 206], [0, 85, 170, 207], [0, 0, 0, 208], [0, 0, 0, 209], [0, 17, 34, 210], [51, 68, 85, 210], [102, 119, 136, 210], [153, 170, 187, 210], [204, 221, 238, 210], [0, 0, 0, 211], [0, 0, 0, 212], [0, 85, 170, 213], [0, 0, 0, 214], [0, 51, 102, 215], [153, 204, 0, 215], [0, 85, 170, 216], [0, 0, 0, 217], [0, 0, 0, 218], [0, 85, 170, 219], [0, 51, 102, 220], [153, 204, 0, 220], [0, 15, 30, 221], [45, 60, 75, 221], [90, 105, 120, 221], [135, 150, 165, 221], [180, 195, 210, 221], [225, 240, 0, 221], [0, 85, 170, 222], [0, 0, 0, 223], [0, 0, 0, 224], [0, 17, 34, 225], [51, 68, 85, 225], [102, 119, 136, 225], [153, 170, 187, 225], [204, 221, 238, 225], [0, 0, 0, 226], [0, 0, 0, 227], [0, 85, 170, 228], [0, 0, 0, 229], [0, 51, 102, 230], [153, 204, 0, 230], [0, 85, 170, 231], [0, 0, 0, 232], [0, 0, 0, 233], [0, 85, 170, 234], [0, 51, 102, 235], [153, 204, 0, 235], [0, 0, 0, 236], [0, 85, 170, 237], [0, 15, 30, 238], [45, 60, 75, 238], [90, 105, 120, 238], [135, 150, 165, 238], [180, 195, 210, 238], [225, 240, 0, 238], [0, 0, 0, 239], [0, 17, 34, 240], [51, 68, 85, 240], [102, 119, 136, 240], [153, 170, 187, 240], [204, 221, 238, 240], [0, 0, 0, 241], [0, 0, 0, 242], [0, 85, 170, 243], [0, 0, 0, 244], [0, 51, 102, 245], [153, 204, 0, 245], [0, 85, 170, 246], [0, 0, 0, 247], [0, 0, 0, 248], [0, 85, 170, 249], [0, 51, 102, 250], [153, 204, 0, 250], [0, 0, 0, 251], [0, 85, 170, 252], [0, 0, 0, 253], [0, 0, 0, 254], [5, 15, 25, 102], [35, 40, 45, 102], [50, 55, 60, 102], [65, 70, 75, 102], [80, 85, 90, 102], [95, 100, 105, 102], [110, 115, 120, 102], [125, 130, 135, 102], [140, 145, 150, 102], [155, 160, 165, 102], [170, 175, 180, 102], [185, 190, 195, 102], [200, 205, 210, 102], [215, 220, 225, 102], [230, 235, 240, 102], [245, 250, 0, 102], [15, 45, 60, 68], [75, 90, 105, 68], [120, 135, 150, 68], [165, 180, 195, 68], [210, 225, 240, 68], [17, 34, 51, 45], [68, 85, 102, 45], [119, 136, 153, 45], [170, 187, 204, 45], [221, 238, 0, 45], [17, 51, 68, 60], [85, 102, 119, 60], [136, 153, 170, 60], [187, 204, 221, 60], [238, 0, 0, 60], [17, 34, 51, 90], [68, 85, 102, 90], [119, 136, 153, 90], [170, 187, 204, 90], [221, 238, 0, 90], [17, 34, 51, 120], [68, 85, 102, 120], [119, 136, 153, 120], [170, 187, 204, 120], [221, 238, 0, 120], [20, 25, 30, 51], [35, 40, 45, 51], [50, 55, 60, 51], [65, 70, 75, 51], [80, 85, 90, 51], [95, 100, 105, 51], [110, 115, 120, 51], [125, 130, 135, 51], [140, 145, 150, 51], [155, 160, 165, 51], [170, 175, 180, 51], [185, 190, 195, 51], [200, 205, 210, 51], [215, 220, 225, 51], [230, 235, 240, 51], [245, 250, 0, 51], [45, 60, 75, 17], [90, 105, 120, 17], [135, 150, 165, 17], [180, 195, 210, 17], [225, 240, 0, 17], [45, 75, 90, 34], [105, 120, 135, 34], [150, 165, 180, 34], [195, 210, 225, 34], [240, 0, 0, 34], [51, 153, 204, 20], [51, 102, 153, 25], [204, 0, 0, 25], [51, 85, 119, 30], [136, 153, 170, 30], [187, 204, 221, 30], [238, 0, 0, 30], [51, 102, 153, 35], [204, 0, 0, 35], [51, 102, 153, 40], [204, 0, 0, 40], [51, 102, 153, 50], [204, 0, 0, 50], [51, 102, 153, 55], [204, 0, 0, 55], [51, 102, 153, 70], [204, 0, 0, 70], [51, 102, 153, 80], [204, 0, 0, 80], [51, 102, 153, 100], [204, 0, 0, 100], [51, 102, 153, 110], [204, 0, 0, 110], [65, 67, 69, 0], [71, 73, 75, 0], [77, 79, 81, 0], [83, 85, 87, 0], [89, 91, 93, 0], [95, 97, 99, 0], [101, 103, 105, 0], [107, 109, 111, 0], [113, 115, 117, 0], [119, 121, 123, 0], [125, 127, 128, 0], [129, 130, 131, 0], [132, 133, 134, 0], [135, 136, 137, 0], [138, 139, 140, 0], [141, 142, 143, 0], [144, 145, 146, 0], [147, 148, 149, 0], [150, 151, 152, 0], [153, 154, 155, 0], [156, 157, 158, 0], [159, 160, 161, 0], [162, 163, 164, 0], [165, 166, 167, 0], [168, 169, 170, 0], [171, 172, 173, 0], [174, 175, 176, 0], [177, 178, 179, 0], [180, 181, 182, 0], [183, 184, 185, 0], [186, 187, 188, 0], [189, 190, 191, 0], [192, 193, 194, 0], [195, 196, 197, 0], [198, 199, 200, 0], [201, 202, 203, 0], [204, 205, 206, 0], [207, 208, 209, 0], [210, 211, 212, 0], [213, 214, 215, 0], [216, 217, 218, 0], [219, 220, 221, 0], [222, 223, 224, 0], [225, 226, 227, 0], [228, 229, 230, 0], [231, 232, 233, 0], [234, 235, 236, 0], [237, 238, 239, 0], [240, 241, 242, 0], [243, 244, 245, 0], [246, 247, 248, 0], [249, 250, 251, 0], [252, 253, 254, 0], [68, 85, 102, 15], [119, 136, 153, 15], [170, 187, 204, 15], [221, 238, 0, 15], [85, 170, 0, 3], [85, 170, 0, 6], [85, 170, 0, 9], [85, 170, 0, 12], [85, 170, 0, 18], [85, 170, 0, 21], [85, 170, 0, 24], [85, 170, 0, 27], [85, 170, 0, 33], [85, 170, 0, 36], [85, 170, 0, 39], [85, 170, 0, 42], [85, 170, 0, 48], [85, 170, 0, 54], [85, 170, 0, 57], [85, 170, 0, 63], [85, 170, 0, 66], [85, 170, 0, 72], [85, 170, 0, 78], [85, 170, 0, 84], [85, 170, 0, 96], [85, 170, 0, 108], [85, 170, 0, 114], [85, 170, 0, 126], [102, 153, 204, 5], [153, 204, 0, 10]];
        let rgb_pixels = vec![[190, 152, 114], [76, 38, 190], [188, 188, 188], [186, 124, 62], [184, 184, 184], [182, 182, 182], [180, 168, 156], [144, 132, 120], [108, 96, 84], [72, 60, 48], [36, 24, 12], [178, 178, 178], [176, 176, 176], [174, 116, 58], [172, 172, 172], [170, 168, 166], [164, 162, 160], [158, 156, 154], [152, 150, 148], [146, 144, 142], [140, 138, 136], [134, 132, 130], [128, 126, 124], [122, 120, 118], [116, 114, 112], [110, 108, 106], [104, 102, 100], [98, 96, 94], [92, 90, 88], [86, 84, 82], [80, 78, 76], [74, 72, 70], [68, 66, 64], [62, 60, 58], [56, 54, 52], [50, 48, 46], [44, 42, 40], [38, 36, 34], [32, 30, 28], [26, 24, 22], [20, 18, 16], [14, 12, 10], [8, 6, 4], [2, 170, 170], [168, 112, 56], [166, 166, 166], [164, 164, 164], [162, 108, 54], [160, 128, 96], [64, 32, 160], [158, 158, 158], [156, 104, 52], [154, 154, 154], [152, 152, 152], [150, 140, 130], [120, 110, 100], [90, 80, 70], [60, 50, 40], [30, 20, 10], [148, 148, 148], [146, 146, 146], [144, 96, 48], [142, 142, 142], [140, 112, 84], [56, 28, 140], [138, 92, 46], [136, 128, 120], [112, 104, 96], [88, 80, 72], [64, 56, 48], [40, 32, 24], [16, 8, 136], [134, 134, 134], [132, 88, 44], [130, 104, 78], [52, 26, 130], [128, 128, 128], [127, 127, 127], [126, 84, 42], [125, 100, 75], [50, 25, 125], [124, 124, 124], [123, 82, 41], [122, 122, 122], [121, 121, 121], [120, 112, 104], [96, 88, 80], [72, 64, 56], [48, 40, 32], [24, 16, 8], [119, 112, 105], [98, 91, 84], [77, 70, 63], [56, 49, 42], [35, 28, 21], [14, 7, 119], [118, 118, 118], [117, 78, 39], [116, 116, 116], [115, 92, 69], [46, 23, 115], [114, 76, 38], [113, 113, 113], [112, 112, 112], [111, 74, 37], [110, 88, 66], [44, 22, 110], [109, 109, 109], [108, 72, 36], [107, 107, 107], [106, 106, 106], [105, 98, 91], [84, 77, 70], [63, 56, 49], [42, 35, 28], [21, 14, 7], [104, 104, 104], [103, 103, 103], [102, 100, 98], [96, 94, 92], [90, 88, 86], [84, 82, 80], [78, 76, 74], [72, 70, 68], [66, 64, 62], [60, 58, 56], [54, 52, 50], [48, 46, 44], [42, 40, 38], [36, 34, 32], [30, 28, 26], [24, 22, 20], [18, 16, 14], [12, 10, 8], [6, 4, 2], [101, 101, 101], [100, 80, 60], [40, 20, 100], [99, 66, 33], [98, 98, 98], [97, 97, 97], [96, 64, 32], [95, 76, 57], [38, 19, 95], [94, 94, 94], [93, 62, 31], [92, 92, 92], [91, 91, 91], [90, 84, 78], [72, 66, 60], [54, 48, 42], [36, 30, 24], [18, 12, 6], [89, 89, 89], [88, 88, 88], [87, 58, 29], [86, 86, 86], [85, 84, 83], [82, 81, 80], [79, 78, 77], [76, 75, 74], [73, 72, 71], [70, 69, 68], [67, 66, 65], [64, 63, 62], [61, 60, 59], [58, 57, 56], [55, 54, 53], [52, 51, 50], [49, 48, 47], [46, 45, 44], [43, 42, 41], [40, 39, 38], [37, 36, 35], [34, 33, 32], [31, 30, 29], [28, 27, 26], [25, 24, 23], [22, 21, 20], [19, 18, 17], [16, 15, 14], [13, 12, 11], [10, 9, 8], [7, 6, 5], [4, 3, 2], [1, 85, 85], [84, 56, 28], [83, 83, 83], [82, 82, 82], [81, 54, 27], [80, 64, 48], [32, 16, 80], [79, 79, 79], [78, 52, 26], [77, 77, 77], [76, 76, 76], [75, 70, 65], [60, 55, 50], [45, 40, 35], [30, 25, 20], [15, 10, 5], [74, 74, 74], [73, 73, 73], [72, 48, 24], [71, 71, 71], [70, 56, 42], [28, 14, 70], [69, 46, 23], [68, 64, 60], [56, 52, 48], [44, 40, 36], [32, 28, 24], [20, 16, 12], [8, 4, 68], [67, 67, 67], [66, 44, 22], [65, 52, 39], [26, 13, 65], [64, 64, 64], [63, 42, 21], [62, 62, 62], [61, 61, 61], [60, 56, 52], [48, 44, 40], [36, 32, 28], [24, 20, 16], [12, 8, 4], [59, 59, 59], [58, 58, 58], [57, 38, 19], [56, 56, 56], [55, 44, 33], [22, 11, 55], [54, 36, 18], [53, 53, 53], [52, 52, 52], [51, 50, 49], [48, 47, 46], [45, 44, 43], [42, 41, 40], [39, 38, 37], [36, 35, 34], [33, 32, 31], [30, 29, 28], [27, 26, 25], [24, 23, 22], [21, 20, 19], [18, 17, 16], [15, 14, 13], [12, 11, 10], [9, 8, 7], [6, 5, 4], [3, 2, 1], [50, 40, 30], [20, 10, 50], [49, 49, 49], [48, 32, 16], [47, 47, 47], [46, 46, 46], [45, 42, 39], [36, 33, 30], [27, 24, 21], [18, 15, 12], [9, 6, 3], [44, 44, 44], [43, 43, 43], [42, 28, 14], [41, 41, 41], [40, 32, 24], [16, 8, 40], [39, 26, 13], [38, 38, 38], [37, 37, 37], [36, 24, 12], [35, 28, 21], [14, 7, 35], [34, 32, 30], [28, 26, 24], [22, 20, 18], [16, 14, 12], [10, 8, 6], [4, 2, 34], [33, 22, 11], [32, 32, 32], [31, 31, 31], [30, 28, 26], [24, 22, 20], [18, 16, 14], [12, 10, 8], [6, 4, 2], [29, 29, 29], [28, 28, 28], [27, 18, 9], [26, 26, 26], [25, 20, 15], [10, 5, 25], [24, 16, 8], [23, 23, 23], [22, 22, 22], [21, 14, 7], [20, 16, 12], [8, 4, 20], [19, 19, 19], [18, 12, 6], [17, 16, 15], [14, 13, 12], [11, 10, 9], [8, 7, 6], [5, 4, 3], [2, 1, 17], [16, 16, 16], [15, 14, 13], [12, 11, 10], [9, 8, 7], [6, 5, 4], [3, 2, 1], [14, 14, 14], [13, 13, 13], [12, 8, 4], [11, 11, 11], [10, 8, 6], [4, 2, 10], [9, 6, 3], [8, 8, 8], [7, 7, 7], [6, 4, 2], [5, 4, 3], [2, 1, 5], [4, 4, 4], [3, 2, 1], [2, 2, 2], [1, 1, 1], [150, 144, 138], [132, 129, 126], [123, 120, 117], [114, 111, 108], [105, 102, 99], [96, 93, 90], [87, 84, 81], [78, 75, 72], [69, 66, 63], [60, 57, 54], [51, 48, 45], [42, 39, 36], [33, 30, 27], [24, 21, 18], [15, 12, 9], [6, 3, 153], [176, 154, 143], [132, 121, 110], [99, 88, 77], [66, 55, 44], [33, 22, 11], [196, 182, 168], [154, 140, 126], [112, 98, 84], [70, 56, 42], [28, 14, 210], [182, 156, 143], [130, 117, 104], [91, 78, 65], [52, 39, 26], [13, 195, 195], [154, 143, 132], [121, 110, 99], [88, 77, 66], [55, 44, 33], [22, 11, 165], [126, 117, 108], [99, 90, 81], [72, 63, 54], [45, 36, 27], [18, 9, 135], [188, 184, 180], [176, 172, 168], [164, 160, 156], [152, 148, 144], [140, 136, 132], [128, 124, 120], [116, 112, 108], [104, 100, 96], [92, 88, 84], [80, 76, 72], [68, 64, 60], [56, 52, 48], [44, 40, 36], [32, 28, 24], [20, 16, 12], [8, 4, 204], [196, 182, 168], [154, 140, 126], [112, 98, 84], [70, 56, 42], [28, 14, 238], [182, 156, 143], [130, 117, 104], [91, 78, 65], [52, 39, 26], [13, 221, 221], [188, 94, 47], [184, 138, 92], [46, 230, 230], [180, 150, 120], [105, 90, 75], [60, 45, 30], [15, 225, 225], [176, 132, 88], [44, 220, 220], [172, 129, 86], [43, 215, 215], [164, 123, 82], [41, 205, 205], [160, 120, 80], [40, 200, 200], [148, 111, 74], [37, 185, 185], [140, 105, 70], [35, 175, 175], [124, 93, 62], [31, 155, 155], [116, 87, 58], [29, 145, 145], [190, 188, 186], [184, 182, 180], [178, 176, 174], [172, 170, 168], [166, 164, 162], [160, 158, 156], [154, 152, 150], [148, 146, 144], [142, 140, 138], [136, 134, 132], [130, 128, 127], [126, 125, 124], [123, 122, 121], [120, 119, 118], [117, 116, 115], [114, 113, 112], [111, 110, 109], [108, 107, 106], [105, 104, 103], [102, 101, 100], [99, 98, 97], [96, 95, 94], [93, 92, 91], [90, 89, 88], [87, 86, 85], [84, 83, 82], [81, 80, 79], [78, 77, 76], [75, 74, 73], [72, 71, 70], [69, 68, 67], [66, 65, 64], [63, 62, 61], [60, 59, 58], [57, 56, 55], [54, 53, 52], [51, 50, 49], [48, 47, 46], [45, 44, 43], [42, 41, 40], [39, 38, 37], [36, 35, 34], [33, 32, 31], [30, 29, 28], [27, 26, 25], [24, 23, 22], [21, 20, 19], [18, 17, 16], [15, 14, 13], [12, 11, 10], [9, 8, 7], [6, 5, 4], [3, 2, 1], [176, 160, 144], [128, 112, 96], [80, 64, 48], [32, 16, 240], [168, 84, 252], [166, 83, 249], [164, 82, 246], [162, 81, 243], [158, 79, 237], [156, 78, 234], [154, 77, 231], [152, 76, 228], [148, 74, 222], [146, 73, 219], [144, 72, 216], [142, 71, 213], [138, 69, 207], [134, 67, 201], [132, 66, 198], [128, 64, 192], [126, 63, 189], [122, 61, 183], [118, 59, 177], [114, 57, 171], [106, 53, 159], [98, 49, 147], [94, 47, 141], [86, 43, 129], [150, 100, 50], [98, 49, 245]];
        for (&cmyk_pixel, rgb_pixel) in cmyk_pixels.iter().zip(rgb_pixels) {
            single_pix_correct(cmyk_pixel, rgb_pixel);
        }
    }

    #[cfg(feature = "benchmarks")]
    #[bench]
    fn bench_cmyk_to_rgb(b: &mut Bencher) {
        let mut v = Vec::with_capacity((W * H * 4) as usize);
        for c in 0..=255 {
            for k in 0..=255 {
                v.push(c as u8);
                v.push(0);
                v.push(0);
                v.push(k as u8);
            }
        }

        b.iter(|| {
            cmyk_to_rgb(&v);
        });
    }

    #[cfg(feature = "benchmarks")]
    #[bench]
    fn bench_cmyk_to_rgb_single(b: &mut Bencher) {
        b.iter(|| {
            cmyk_to_rgb(&[128, 128, 128, 128]);
        });
    }

}
