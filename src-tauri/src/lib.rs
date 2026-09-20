use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{Arc, Mutex},
    time::Duration,
};
use wait_timeout::ChildExt;

const MAX_EXPRESSION_LENGTH: usize = 20_000;
const RENDER_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RenderRequest {
    expression: String,
    font_size_pt: f32,
    color: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RenderResult {
    svg: String,
    width: f32,
    height: f32,
    cache_key: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TypstStatus {
    available: bool,
    version: Option<String>,
    executable_path: Option<String>,
    error: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CopyResult {
    format: CopyFormat,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum CopyFormat {
    Svg,
    Png,
}

#[derive(Debug, Clone, Serialize)]
pub struct AppError {
    kind: &'static str,
    message: String,
    diagnostic: Option<String>,
}

impl AppError {
    fn new(kind: &'static str, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
            diagnostic: None,
        }
    }

    fn diagnostic(kind: &'static str, message: impl Into<String>, diagnostic: String) -> Self {
        Self {
            kind,
            message: message.into(),
            diagnostic: Some(diagnostic),
        }
    }
}

#[derive(Clone)]
struct CachedRender {
    key: String,
    svg: Vec<u8>,
    png: Option<Vec<u8>>,
}

#[derive(Clone, Default)]
pub struct AppState {
    cache: Arc<Mutex<Option<CachedRender>>>,
}

#[tauri::command]
fn get_typst_status() -> TypstStatus {
    match typst_executable() {
        Some(path) => match Command::new(&path).arg("--version").output() {
            Ok(output) if output.status.success() => TypstStatus {
                available: true,
                version: Some(String::from_utf8_lossy(&output.stdout).trim().to_owned()),
                executable_path: Some(path.display().to_string()),
                error: None,
            },
            Ok(output) => TypstStatus {
                available: false,
                version: None,
                executable_path: Some(path.display().to_string()),
                error: Some(String::from_utf8_lossy(&output.stderr).trim().to_owned()),
            },
            Err(error) => unavailable_status(error.to_string()),
        },
        None => unavailable_status(
            "Typstをインストールし、typst.exeがPATHに含まれるよう設定してからアプリを再起動してください。".into(),
        ),
    }
}

fn unavailable_status(error: String) -> TypstStatus {
    TypstStatus {
        available: false,
        version: None,
        executable_path: None,
        error: Some(error),
    }
}

#[tauri::command]
async fn render_equation(
    request: RenderRequest,
    state: tauri::State<'_, AppState>,
) -> Result<RenderResult, AppError> {
    validate_request(&request)?;
    let app_state = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || render_svg(request, &app_state))
        .await
        .map_err(|error| {
            AppError::new(
                "internal",
                format!("レンダリング処理に失敗しました: {error}"),
            )
        })?
}

#[tauri::command]
async fn copy_equation(
    request: RenderRequest,
    format: CopyFormat,
    state: tauri::State<'_, AppState>,
) -> Result<CopyResult, AppError> {
    validate_request(&request)?;
    let app_state = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || render_and_copy(request, format, &app_state))
        .await
        .map_err(|error| AppError::new("internal", format!("コピー処理に失敗しました: {error}")))?
}

fn validate_request(request: &RenderRequest) -> Result<(), AppError> {
    if request.expression.trim().is_empty() {
        return Err(AppError::new("validation", "数式を入力してください。"));
    }
    if request.expression.chars().count() > MAX_EXPRESSION_LENGTH {
        return Err(AppError::new(
            "validation",
            "数式は20,000文字以内で入力してください。",
        ));
    }
    if !request.font_size_pt.is_finite() || !(8.0..=144.0).contains(&request.font_size_pt) {
        return Err(AppError::new(
            "validation",
            "フォントサイズは8～144ptで指定してください。",
        ));
    }
    let valid_color = request.color.len() == 7
        && request.color.starts_with('#')
        && request.color[1..].chars().all(|c| c.is_ascii_hexdigit());
    if !valid_color {
        return Err(AppError::new(
            "validation",
            "文字色は#RRGGBB形式で指定してください。",
        ));
    }
    Ok(())
}

fn render_svg(request: RenderRequest, state: &AppState) -> Result<RenderResult, AppError> {
    let key = cache_key(&request);
    if let Some(cached) = cached_for(state, &key) {
        return result_from_svg(cached.svg, key);
    }

    let source = build_source(&request);
    let svg = compile_typst(&source, OutputFormat::Svg)?;
    *state.cache.lock().expect("cache mutex poisoned") = Some(CachedRender {
        key: key.clone(),
        svg: svg.clone(),
        png: None,
    });
    result_from_svg(svg, key)
}

fn render_and_copy(
    request: RenderRequest,
    format: CopyFormat,
    state: &AppState,
) -> Result<CopyResult, AppError> {
    let key = cache_key(&request);
    let source = build_source(&request);
    let mut cached = cached_for(state, &key).unwrap_or(CachedRender {
        key: key.clone(),
        svg: Vec::new(),
        png: None,
    });
    match format {
        CopyFormat::Svg if cached.svg.is_empty() => {
            cached.svg = compile_typst(&source, OutputFormat::Svg)?;
        }
        CopyFormat::Png if cached.png.is_none() => {
            cached.png = Some(compile_typst(&source, OutputFormat::Png)?);
        }
        _ => {}
    }
    clipboard::copy_images(&cached.svg, cached.png.as_deref(), format)?;
    *state.cache.lock().expect("cache mutex poisoned") = Some(cached);
    Ok(CopyResult { format })
}

fn cached_for(state: &AppState, key: &str) -> Option<CachedRender> {
    state
        .cache
        .lock()
        .expect("cache mutex poisoned")
        .as_ref()
        .filter(|item| item.key == key)
        .cloned()
}

fn cache_key(request: &RenderRequest) -> String {
    let mut hasher = Sha256::new();
    hasher.update(request.expression.as_bytes());
    hasher.update(request.font_size_pt.to_le_bytes());
    hasher.update(request.color.as_bytes());
    format!("{:x}", hasher.finalize())
}

fn build_source(request: &RenderRequest) -> String {
    let size = request.font_size_pt;
    let color = &request.color;
    let expression = &request.expression;
    format!(
        "#set page(width: auto, height: auto, margin: 4pt, fill: none)\n#set text(size: {size}pt, fill: rgb(\"{color}\"))\n$ {expression} $\n"
    )
}

enum OutputFormat {
    Svg,
    Png,
}

impl OutputFormat {
    fn extension(&self) -> &'static str {
        match self {
            Self::Svg => "svg",
            Self::Png => "png",
        }
    }
}

fn compile_typst(source: &str, format: OutputFormat) -> Result<Vec<u8>, AppError> {
    let typst = typst_executable()
        .ok_or_else(|| AppError::new("typst_missing", "TypstがPATH上に見つかりません。"))?;
    let temp = tempfile::tempdir()
        .map_err(|e| AppError::new("io", format!("一時フォルダーを作成できません: {e}")))?;
    let input = temp.path().join("equation.typ");
    let output = temp.path().join(format!("equation.{}", format.extension()));
    fs::write(&input, source)
        .map_err(|e| AppError::new("io", format!("一時ファイルを書き込めません: {e}")))?;

    let mut command = Command::new(typst);
    command
        .arg("compile")
        .arg("--root")
        .arg(temp.path())
        .arg("--diagnostic-format")
        .arg("short");
    if matches!(format, OutputFormat::Png) {
        command.arg("--ppi").arg("300");
    }
    let mut child = command
        .arg(&input)
        .arg(&output)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| AppError::new("typst_start", format!("Typstを起動できません: {e}")))?;

    match child
        .wait_timeout(RENDER_TIMEOUT)
        .map_err(|e| AppError::new("typst_wait", e.to_string()))?
    {
        Some(_) => {
            let process_output = child
                .wait_with_output()
                .map_err(|e| AppError::new("typst_wait", e.to_string()))?;
            if !process_output.status.success() {
                return Err(AppError::diagnostic(
                    "typst_compile",
                    "数式にTypstのエラーがあります。",
                    clean_diagnostic(
                        &String::from_utf8_lossy(&process_output.stderr),
                        temp.path(),
                    ),
                ));
            }
        }
        None => {
            let _ = child.kill();
            let _ = child.wait();
            return Err(AppError::new(
                "timeout",
                "レンダリングが5秒を超えたため中止しました。",
            ));
        }
    }
    fs::read(output).map_err(|e| AppError::new("io", format!("Typstの出力を読み込めません: {e}")))
}

fn clean_diagnostic(diagnostic: &str, root: &Path) -> String {
    diagnostic.replace(&root.display().to_string(), "")
}

fn result_from_svg(svg: Vec<u8>, key: String) -> Result<RenderResult, AppError> {
    let svg = String::from_utf8(svg)
        .map_err(|_| AppError::new("svg", "Typstから不正なSVGが返されました。"))?;
    let (width, height) = svg_dimensions(&svg).unwrap_or((0.0, 0.0));
    Ok(RenderResult {
        svg,
        width,
        height,
        cache_key: key,
    })
}

fn svg_dimensions(svg: &str) -> Option<(f32, f32)> {
    let marker = "viewBox=\"";
    let start = svg.find(marker)? + marker.len();
    let end = svg[start..].find('"')? + start;
    let values: Vec<f32> = svg[start..end]
        .split_whitespace()
        .filter_map(|v| v.parse().ok())
        .collect();
    (values.len() == 4).then(|| (values[2], values[3]))
}

fn typst_executable() -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    for directory in std::env::split_paths(&path) {
        #[cfg(windows)]
        let candidate = directory.join("typst.exe");
        #[cfg(not(windows))]
        let candidate = directory.join("typst");
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

#[cfg(windows)]
mod clipboard {
    use super::AppError;
    use image::ImageReader;
    use std::{io::Cursor, ptr};
    use windows_sys::Win32::{
        Foundation::{GlobalFree, HANDLE},
        System::{
            DataExchange::{
                CloseClipboard, EmptyClipboard, OpenClipboard, RegisterClipboardFormatW,
                SetClipboardData,
            },
            Memory::{GlobalAlloc, GlobalLock, GlobalUnlock, GMEM_MOVEABLE},
        },
    };

    const CF_DIBV5: u32 = 17;

    struct ClipboardGuard;
    impl Drop for ClipboardGuard {
        fn drop(&mut self) {
            unsafe {
                CloseClipboard();
            }
        }
    }

    pub fn copy_images(
        svg: &[u8],
        png: Option<&[u8]>,
        format: super::CopyFormat,
    ) -> Result<(), AppError> {
        let svg_format = register_format("image/svg+xml")?;
        let png_format = register_format("PNG")?;
        unsafe {
            if OpenClipboard(ptr::null_mut()) == 0 {
                return Err(AppError::new(
                    "clipboard_busy",
                    "クリップボードが使用中です。少し待ってから再試行してください。",
                ));
            }
            let _guard = ClipboardGuard;
            if EmptyClipboard() == 0 {
                return Err(AppError::new(
                    "clipboard",
                    "クリップボードを初期化できません。",
                ));
            }
            match format {
                super::CopyFormat::Svg => set_data(svg_format, svg)?,
                super::CopyFormat::Png => {
                    let png =
                        png.ok_or_else(|| AppError::new("png", "PNGを生成できませんでした。"))?;
                    let dib = png_to_dibv5(png)?;
                    set_data(png_format, png)?;
                    set_data(CF_DIBV5, &dib)?;
                }
            }
        }
        Ok(())
    }

    fn register_format(name: &str) -> Result<u32, AppError> {
        let wide: Vec<u16> = name.encode_utf16().chain(Some(0)).collect();
        let format = unsafe { RegisterClipboardFormatW(wide.as_ptr()) };
        if format == 0 {
            Err(AppError::new(
                "clipboard",
                format!("クリップボード形式 {name} を登録できません。"),
            ))
        } else {
            Ok(format)
        }
    }

    unsafe fn set_data(format: u32, bytes: &[u8]) -> Result<(), AppError> {
        let memory = GlobalAlloc(GMEM_MOVEABLE, bytes.len());
        if memory.is_null() {
            return Err(AppError::new(
                "clipboard",
                "クリップボード用メモリを確保できません。",
            ));
        }
        let destination = GlobalLock(memory) as *mut u8;
        if destination.is_null() {
            GlobalFree(memory);
            return Err(AppError::new(
                "clipboard",
                "クリップボード用メモリをロックできません。",
            ));
        }
        ptr::copy_nonoverlapping(bytes.as_ptr(), destination, bytes.len());
        GlobalUnlock(memory);
        if SetClipboardData(format, memory as HANDLE).is_null() {
            GlobalFree(memory);
            return Err(AppError::new(
                "clipboard",
                "クリップボードへデータを書き込めません。",
            ));
        }
        Ok(())
    }

    fn png_to_dibv5(png: &[u8]) -> Result<Vec<u8>, AppError> {
        let image = ImageReader::new(Cursor::new(png))
            .with_guessed_format()
            .map_err(|e| AppError::new("png", format!("PNGを読み込めません: {e}")))?
            .decode()
            .map_err(|e| AppError::new("png", format!("PNGを展開できません: {e}")))?
            .to_rgba8();
        let (width, height) = image.dimensions();
        let byte_count = width
            .checked_mul(height)
            .and_then(|n| n.checked_mul(4))
            .ok_or_else(|| AppError::new("png", "画像サイズが大きすぎます。"))?;
        let mut output = vec![0u8; 124 + byte_count as usize];
        write_u32(&mut output, 0, 124);
        write_i32(&mut output, 4, width as i32);
        write_i32(&mut output, 8, -(height as i32));
        write_u16(&mut output, 12, 1);
        write_u16(&mut output, 14, 32);
        write_u32(&mut output, 16, 3);
        write_u32(&mut output, 20, byte_count);
        write_i32(&mut output, 24, 11_811);
        write_i32(&mut output, 28, 11_811);
        write_u32(&mut output, 40, 0x00ff_0000);
        write_u32(&mut output, 44, 0x0000_ff00);
        write_u32(&mut output, 48, 0x0000_00ff);
        write_u32(&mut output, 52, 0xff00_0000);
        write_u32(&mut output, 56, 0x7352_4742);
        write_u32(&mut output, 108, 4);
        for (source, destination) in image.pixels().zip(output[124..].chunks_exact_mut(4)) {
            destination.copy_from_slice(&[source[2], source[1], source[0], source[3]]);
        }
        Ok(output)
    }

    fn write_u16(buffer: &mut [u8], offset: usize, value: u16) {
        buffer[offset..offset + 2].copy_from_slice(&value.to_le_bytes());
    }
    fn write_u32(buffer: &mut [u8], offset: usize, value: u32) {
        buffer[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
    }
    fn write_i32(buffer: &mut [u8], offset: usize, value: i32) {
        buffer[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        #[test]
        fn dib_header_is_valid() {
            let image = image::RgbaImage::from_pixel(2, 3, image::Rgba([1, 2, 3, 255]));
            let mut png = Vec::new();
            image
                .write_to(&mut Cursor::new(&mut png), image::ImageFormat::Png)
                .unwrap();
            let dib = png_to_dibv5(&png).unwrap();
            assert_eq!(u32::from_le_bytes(dib[0..4].try_into().unwrap()), 124);
            assert_eq!(i32::from_le_bytes(dib[4..8].try_into().unwrap()), 2);
            assert_eq!(i32::from_le_bytes(dib[8..12].try_into().unwrap()), -3);
            assert_eq!(&dib[124..128], &[3, 2, 1, 255]);
        }
    }
}

#[cfg(not(windows))]
mod clipboard {
    use super::AppError;
    pub fn copy_images(_: &[u8], _: Option<&[u8]>, _: super::CopyFormat) -> Result<(), AppError> {
        Err(AppError::new(
            "unsupported",
            "クリップボードへのコピーはWindowsでのみ利用できます。",
        ))
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .manage(AppState::default())
        .invoke_handler(tauri::generate_handler![
            get_typst_status,
            render_equation,
            copy_equation
        ])
        .run(tauri::generate_context!())
        .expect("error while running Typst Equation Clipboard");
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(expression: &str) -> RenderRequest {
        RenderRequest {
            expression: expression.into(),
            font_size_pt: 32.0,
            color: "#112233".into(),
        }
    }

    #[test]
    fn source_wraps_expression_and_style() {
        let source = build_source(&request("x^2 + y^2"));
        assert!(source.contains("size: 32pt"));
        assert!(source.contains("rgb(\"#112233\")"));
        assert!(source.contains("$ x^2 + y^2 $"));
        assert!(source.contains("fill: none"));
    }

    #[test]
    fn validation_rejects_invalid_values() {
        assert!(validate_request(&request("")).is_err());
        let mut invalid = request("x");
        invalid.font_size_pt = 200.0;
        assert!(validate_request(&invalid).is_err());
        invalid.font_size_pt = 32.0;
        invalid.color = "red".into();
        assert!(validate_request(&invalid).is_err());
    }

    #[test]
    fn parses_svg_viewbox() {
        assert_eq!(
            svg_dimensions(r#"<svg viewBox="0 0 42.5 17"></svg>"#),
            Some((42.5, 17.0))
        );
    }

    #[test]
    fn installed_typst_renders_svg_and_png() {
        if typst_executable().is_none() {
            eprintln!("Typst is not installed; skipping integration assertion");
            return;
        }
        let source = build_source(&request("integral_0^infinity e^(-x^2) dif x"));
        let svg = compile_typst(&source, OutputFormat::Svg).unwrap();
        let png = compile_typst(&source, OutputFormat::Png).unwrap();
        assert!(svg.starts_with(b"<svg"));
        assert!(png.starts_with(&[0x89, b'P', b'N', b'G']));
    }
}
