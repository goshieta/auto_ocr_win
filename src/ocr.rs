use windows::Graphics::Imaging::{
    BitmapPixelFormat, SoftwareBitmap,
};
use windows::Media::Ocr::OcrEngine;
use windows::Storage::Streams::{DataWriter, InMemoryRandomAccessStream};
use windows::Win32::System::Com::{CoInitializeEx, CoUninitialize, COINIT_MULTITHREADED};

use crate::capture::CapturedImage;

pub struct OcrEngineWrapper {
    engine: OcrEngine,
}

impl OcrEngineWrapper {
    /// Initialize COM and create the OCR engine.
    /// Returns an error if COM init or OCR engine creation fails.
    pub fn new() -> Result<Self, String> {
        // Initialize COM for this thread (needed for Windows.Media.Ocr)
        unsafe {
            CoInitializeEx(None, COINIT_MULTITHREADED)
                .ok()
                .map_err(|e| format!("CoInitializeEx failed: {:?}", e))?;
        }

        let engine = OcrEngine::TryCreateFromUserProfileLanguages()
            .map_err(|e| format!("TryCreateFromUserProfileLanguages failed: {:?}", e))?;

        Ok(OcrEngineWrapper { engine })
    }

    /// Run OCR on the captured image data and return recognized text.
    /// The input data is BGRA pixel data from GDI capture.
    /// 
    /// Note: This must be called from the same thread that created the OcrEngineWrapper,
    /// because COM is single-threaded. The main loop calls this from the main thread.
    pub fn recognize(&self, image: &CapturedImage) -> Result<String, String> {
        // Convert raw BGRA pixel data to a Windows SoftwareBitmap
        let software_bitmap = create_software_bitmap(image)?;

        // Run OCR
        let result = self
            .engine
            .RecognizeAsync(&software_bitmap)
            .map_err(|e| format!("OCR RecognizeAsync failed: {:?}", e))?
            .get()
            .map_err(|e| format!("OCR get result failed: {:?}", e))?;

        let text = result
            .Text()
            .map_err(|e| format!("Failed to get OCR text: {:?}", e))?
            .to_string();

        // Explicitly close the SoftwareBitmap to release resources
        let _ = software_bitmap.Close();

        Ok(text)
    }
}

impl Drop for OcrEngineWrapper {
    fn drop(&mut self) {
        unsafe {
            CoUninitialize();
        }
    }
}

/// Create a Windows SoftwareBitmap from raw BGRA pixel data.
fn create_software_bitmap(image: &CapturedImage) -> Result<SoftwareBitmap, String> {
    let width = image.width;
    let height = image.height;

    // Create an empty SoftwareBitmap in Bgra8 format
    let bitmap = SoftwareBitmap::Create(
        BitmapPixelFormat::Bgra8,
        width,
        height,
    ).map_err(|e| format!("SoftwareBitmap::Create failed: {:?}", e))?;

    // Create an in-memory random access stream and write pixel data into it
    let stream = InMemoryRandomAccessStream::new()
        .map_err(|e| format!("InMemoryRandomAccessStream::new failed: {:?}", e))?;
    let output_stream = stream
        .GetOutputStreamAt(0)
        .map_err(|e| format!("GetOutputStreamAt failed: {:?}", e))?;

    let data_writer = DataWriter::CreateDataWriter(&output_stream)
        .map_err(|e| format!("DataWriter::CreateDataWriter failed: {:?}", e))?;
    data_writer
        .WriteBytes(&image.data)
        .map_err(|e| format!("DataWriter::WriteBytes failed: {:?}", e))?;
    data_writer
        .StoreAsync()
        .map_err(|e| format!("DataWriter::StoreAsync failed: {:?}", e))?
        .get()
        .map_err(|e| format!("StoreAsync get failed: {:?}", e))?;

    // Get the input stream and read into a Buffer
    let input_stream = stream
        .GetInputStreamAt(0)
        .map_err(|e| format!("GetInputStreamAt failed: {:?}", e))?;

    let reader = windows::Storage::Streams::DataReader::CreateDataReader(&input_stream)
        .map_err(|e| format!("DataReader::CreateDataReader failed: {:?}", e))?;
    reader
        .LoadAsync(image.data.len() as u32)
        .map_err(|e| format!("DataReader::LoadAsync failed: {:?}", e))?
        .get()
        .map_err(|e| format!("LoadAsync get failed: {:?}", e))?;

    let read_buffer = reader
        .ReadBuffer(image.data.len() as u32)
        .map_err(|e| format!("DataReader::ReadBuffer failed: {:?}", e))?;

    // Copy from the buffer into the SoftwareBitmap
    bitmap
        .CopyFromBuffer(&read_buffer)
        .map_err(|e| format!("SoftwareBitmap::CopyFromBuffer failed: {:?}", e))?;

    Ok(bitmap)
}