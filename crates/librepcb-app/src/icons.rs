//! Icons set from the backend (upstream loads them from the Qt resources
//! `:/img/...`). The files are part of the copied UI resources
//! (`ui/resources/img`, see `ui/PROVENANCE.md`) and embedded into the
//! binary.

use slint::{Image, Rgba8Pixel, SharedPixelBuffer};

/// An embedded icon.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Icon {
    /// The application icon (`img/app/librepcb.png`), used for projects.
    App,
    /// A folder (`img/places/folder.png`).
    Folder,
    /// A project folder (`img/places/project_folder.png`).
    ProjectFolder,
    /// A file (`img/places/file.png`).
    File,
}

impl Icon {
    fn data(self) -> &'static [u8] {
        match self {
            Self::App => include_bytes!("../ui/resources/img/app/librepcb.png"),
            Self::Folder => include_bytes!("../ui/resources/img/places/folder.png"),
            Self::ProjectFolder => {
                include_bytes!("../ui/resources/img/places/project_folder.png")
            }
            Self::File => include_bytes!("../ui/resources/img/places/file.png"),
        }
    }

    /// The icon as Slint image (decoded once per thread).
    pub fn image(self) -> Image {
        thread_local! {
            static CACHE: std::cell::RefCell<Vec<(Icon, Image)>> =
                const { std::cell::RefCell::new(Vec::new()) };
        }
        CACHE.with(|cache| {
            if let Some((_, img)) = cache.borrow().iter().find(|(i, _)| *i == self) {
                return img.clone();
            }
            let img = decode_png(self.data()).unwrap_or_default();
            cache.borrow_mut().push((self, img.clone()));
            img
        })
    }
}

/// Decodes a PNG into an RGBA image.
pub fn decode_png(data: &[u8]) -> Option<Image> {
    let mut decoder = png::Decoder::new(std::io::Cursor::new(data));
    decoder.set_transformations(png::Transformations::normalize_to_color8());
    let mut reader = decoder.read_info().ok()?;
    let mut buf = vec![0; reader.output_buffer_size()?];
    let info = reader.next_frame(&mut buf).ok()?;
    let bytes = &buf[..info.buffer_size()];
    let rgba: Vec<u8> = match info.color_type {
        png::ColorType::Rgba => bytes.to_vec(),
        png::ColorType::Rgb => bytes
            .as_chunks::<3>()
            .0
            .iter()
            .flat_map(|c| [c[0], c[1], c[2], 255])
            .collect(),
        png::ColorType::GrayscaleAlpha => bytes
            .as_chunks::<2>()
            .0
            .iter()
            .flat_map(|c| [c[0], c[0], c[0], c[1]])
            .collect(),
        png::ColorType::Grayscale => bytes.iter().flat_map(|&g| [g, g, g, 255]).collect(),
        png::ColorType::Indexed => return None,
    };
    let buffer = SharedPixelBuffer::<Rgba8Pixel>::clone_from_slice(&rgba, info.width, info.height);
    Some(Image::from_rgba8(buffer))
}
