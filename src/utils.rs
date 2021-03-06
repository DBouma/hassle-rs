use crate::os::{SysFreeString, BSTR, HRESULT, LPSTR, LPWSTR, WCHAR};
use crate::{dxil::Dxil, Dxc, DxcIncludeHandler};
use thiserror::Error;

#[cfg(windows)]
use winapi::um::oleauto::SysStringLen;

pub(crate) fn to_wide(msg: &str) -> Vec<WCHAR> {
    widestring::WideCString::from_str(msg).unwrap().into_vec()
}

pub(crate) fn from_wide(wide: LPWSTR) -> String {
    unsafe {
        widestring::WideCStr::from_ptr_str(wide)
            .to_string()
            .expect("widestring decode failed")
    }
}

#[cfg(windows)]
pub(crate) fn from_bstr(string: BSTR) -> String {
    unsafe {
        let len = SysStringLen(string) as usize;

        let result = widestring::WideCStr::from_ptr_with_nul(string, len)
            .to_string()
            .expect("widestring decode failed");

        SysFreeString(string);
        result
    }
}

#[cfg(not(windows))]
pub(crate) fn from_bstr(string: BSTR) -> String {
    // TODO (Marijn): This does NOT cover embedded NULLs

    // BSTR contains its size in the four bytes preceding the pointer, in order to contain NULL bytes:
    // https://docs.microsoft.com/en-us/previous-versions/windows/desktop/automat/bstr
    // DXC on non-Windows does not adhere to that and simply allocates a buffer without prepending the size:
    // https://github.com/microsoft/DirectXShaderCompiler/blob/a8d9780046cb64a1cea842fa6fc28a250e3e2c09/include/dxc/Support/WinAdapter.h#L49-L50
    let result = from_wide(string as LPWSTR);

    unsafe { SysFreeString(string) };
    result
}

pub(crate) fn from_lpstr(string: LPSTR) -> String {
    unsafe {
        let len = (0..).take_while(|&i| *string.offset(i) != 0).count();

        let slice: &[u8] = std::slice::from_raw_parts(string as *const u8, len);
        std::str::from_utf8(slice).map(|s| s.to_owned()).unwrap()
    }
}

struct DefaultIncludeHandler {}

impl DxcIncludeHandler for DefaultIncludeHandler {
    fn load_source(&self, filename: String) -> Option<String> {
        use std::io::Read;
        match std::fs::File::open(filename) {
            Ok(mut f) => {
                let mut content = String::new();
                f.read_to_string(&mut content).unwrap();
                Some(content)
            }
            Err(_) => None,
        }
    }
}

#[derive(Error, Debug)]
pub enum HassleError {
    #[error("Win32 error: {0:X}")]
    Win32Error(HRESULT),
    #[error("Compile error: {0}")]
    CompileError(String),
    #[error("Validation error: {0}")]
    ValidationError(String),
    #[error("Failed to load library {filename:?}: {inner:?}")]
    LoadLibraryError {
        filename: String,
        #[source]
        inner: libloading::Error,
    },
    #[error("LibLoading error: {0:?}")]
    LibLoadingError(#[from] libloading::Error),
    #[error("Utf8 error: {0:?}")]
    Utf8Error(#[from] std::str::Utf8Error),
}

/// Helper function to directly compile a HLSL shader to an intermediate language,
/// this function expects `dxcompiler.dll` to be available in the current
/// executable environment.
///
/// Specify -spirv as one of the `args` to compile to SPIR-V
pub fn compile_hlsl(
    source_name: &str,
    shader_text: &str,
    entry_point: &str,
    target_profile: &str,
    args: &[&str],
    defines: &[(&str, Option<&str>)],
) -> Result<Vec<u8>, HassleError> {
    let dxc = Dxc::new()?;

    let compiler = dxc.create_compiler()?;
    let library = dxc.create_library()?;

    let blob = library
        .create_blob_with_encoding_from_str(shader_text)
        .map_err(HassleError::Win32Error)?;

    let result = compiler.compile(
        &blob,
        source_name,
        entry_point,
        target_profile,
        args,
        Some(Box::new(DefaultIncludeHandler {})),
        defines,
    );

    match result {
        Err(result) => {
            let error_blob = result
                .0
                .get_error_buffer()
                .map_err(HassleError::Win32Error)?;
            Err(HassleError::CompileError(
                library.get_blob_as_string(&error_blob),
            ))
        }
        Ok(result) => {
            let result_blob = result.get_result().map_err(HassleError::Win32Error)?;

            Ok(result_blob.to_vec())
        }
    }
}

/// Helper function to validate a DXIL binary independant from the compilation process,
/// this function expects `dxcompiler.dll` and `dxil.dll` to be available in the current
/// execution environment.
/// `dxil.dll` is currently not available on Linux.
pub fn validate_dxil(data: &[u8]) -> Result<Vec<u8>, HassleError> {
    let dxc = Dxc::new()?;
    let dxil = Dxil::new()?;

    let validator = dxil.create_validator()?;
    let library = dxc.create_library()?;

    let blob_encoding = library
        .create_blob_with_encoding(&data)
        .map_err(HassleError::Win32Error)?;

    match validator.validate(blob_encoding.into()) {
        Ok(blob) => Ok(blob.to_vec()),
        Err(result) => {
            let error_blob = result
                .0
                .get_error_buffer()
                .map_err(HassleError::Win32Error)?;
            Err(HassleError::ValidationError(
                library.get_blob_as_string(&error_blob),
            ))
        }
    }
}
