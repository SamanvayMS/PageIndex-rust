//! Raw PDFium access through pdfium-render's bindings.
//!
//! pdfium-render 0.9 keeps its handle accessors crate-private, so the extractor owns the raw
//! bindings and drives PDFium directly (docs/spikes/A-pdfium.md). PDFium must be build 7999 for
//! parity with pypdfium2 5.13.0.
#![allow(unsafe_code)]

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use anyhow::{Context, Result, anyhow, bail};
use pdfium_render::prelude::*;

static BINDINGS: OnceLock<Box<dyn PdfiumLibraryBindings>> = OnceLock::new();

/// PDFium is not thread-safe, not even across separate documents (shared font caches and
/// globals). Every [`Document`] holds this lock for its lifetime; it is reentrant so one thread
/// may open nested documents. Raw-binding users outside `Document` must hold [`lock`] too.
static PDFIUM_LOCK: parking_lot::ReentrantMutex<()> = parking_lot::ReentrantMutex::new(());

/// Hold the process-wide PDFium lock (reentrant on the same thread).
pub fn lock() -> parking_lot::ReentrantMutexGuard<'static, ()> {
    PDFIUM_LOCK.lock()
}

/// Candidate library locations: `$PDFIUM_LIB` (file or directory), next to the executable,
/// `./pdfium/lib`, then the system search path.
fn candidates() -> Vec<PathBuf> {
    let name = Pdfium::pdfium_platform_library_name();
    let mut out = Vec::new();
    if let Ok(p) = std::env::var("PDFIUM_LIB") {
        let p = PathBuf::from(p);
        out.push(if p.is_dir() { p.join(&name) } else { p });
    }
    if let Some(dir) = std::env::current_exe()
        .ok()
        .and_then(|e| e.parent().map(Path::to_path_buf))
    {
        out.push(dir.join(&name));
    }
    out.push(PathBuf::from("pdfium/lib").join(&name));
    out
}

/// The process-wide PDFium bindings (loaded and initialised once).
pub fn bindings() -> Result<&'static dyn PdfiumLibraryBindings> {
    let _guard = lock();
    if let Some(b) = BINDINGS.get() {
        return Ok(b.as_ref());
    }
    let mut last_err = None;
    let mut loaded = None;
    for path in candidates() {
        if !path.exists() {
            continue;
        }
        match Pdfium::bind_to_library(&path) {
            Ok(b) => {
                loaded = Some(b);
                break;
            }
            Err(e) => last_err = Some(anyhow!("{}: {e:?}", path.display())),
        }
    }
    let b = match loaded {
        Some(b) => b,
        None => Pdfium::bind_to_system_library().map_err(|e| {
            anyhow!("PDFium not found (set PDFIUM_LIB): {e:?}; last error: {last_err:?}")
        })?,
    };
    // SAFETY: FPDF_InitLibrary is called exactly once, before any other PDFium call.
    unsafe { b.FPDF_InitLibrary() };
    let _ = BINDINGS.set(b);
    Ok(BINDINGS.get().context("bindings")?.as_ref())
}

/// An open PDFium document; pages opened through it stay open until the document drops
/// (the reference keeps every page alive through pass 2 so FPDF_FONT addresses stay unique).
pub struct Document {
    pub b: &'static dyn PdfiumLibraryBindings,
    pub doc: FPDF_DOCUMENT,
    pub pages: Vec<FPDF_PAGE>,
    _bytes: Vec<u8>,
    // Declared last: released only after the handles above are closed in `drop`.
    _lock: parking_lot::ReentrantMutexGuard<'static, ()>,
}

impl Document {
    pub fn open(bytes: Vec<u8>) -> Result<Self> {
        let lock = lock();
        let b = bindings()?;
        // SAFETY: `bytes` is owned by the Document and outlives the FPDF_DOCUMENT.
        let doc = unsafe { b.FPDF_LoadMemDocument64(&bytes, None) };
        if doc.is_null() {
            // SAFETY: plain error query.
            let code = unsafe { b.FPDF_GetLastError() };
            bail!("PDFium could not open the document (error {code})");
        }
        Ok(Self {
            b,
            doc,
            pages: Vec::new(),
            _bytes: bytes,
            _lock: lock,
        })
    }

    pub fn page_count(&self) -> usize {
        // SAFETY: valid document handle.
        unsafe { self.b.FPDF_GetPageCount(self.doc) }.max(0) as usize
    }

    /// Load page `idx` and keep it open for the document's lifetime.
    pub fn load_page(&mut self, idx: usize) -> Result<FPDF_PAGE> {
        // SAFETY: valid document handle and in-range index.
        let page = unsafe { self.b.FPDF_LoadPage(self.doc, idx as i32) };
        if page.is_null() {
            bail!("FPDF_LoadPage({idx}) failed");
        }
        self.pages.push(page);
        Ok(page)
    }
}

impl Drop for Document {
    fn drop(&mut self) {
        // SAFETY: handles were produced by this document and are closed exactly once.
        unsafe {
            for &p in &self.pages {
                self.b.FPDF_ClosePage(p);
            }
            self.b.FPDF_CloseDocument(self.doc);
        }
    }
}
