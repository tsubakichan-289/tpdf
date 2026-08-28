use std::path::PathBuf;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PdfDocument {
    pub path: PathBuf,
    pub page_count: usize,
}

impl PdfDocument {
    pub fn new(path: PathBuf) -> Self {
        Self {
            path,
            page_count: 0,
        }
    }
}
