//! 构建脚本
//!
//! 1. `tauri_build::build()`：tauri 的常规构建步骤（**含为应用 bin 生成并嵌入
//!    Windows 清单**）。
//! 2. `embed_common_controls_manifest_for_tests()`：**只为测试/基准产物**
//!    （`tests/*`、`benches/*`）嵌入同一份 Common Controls v6 清单。
//!
//! 为什么需要第 2 步：`tauri` 的 UI 依赖链（`tauri-runtime-wry` / `muda` / `rfd`）
//! 会静态导入 `comctl32.dll!TaskDialogIndirect`，该导出**只在 Common Controls v6
//! 中存在**（`%WINDIR%\System32\comctl32.dll` 是 5.82，没有它）。是否激活 v6 由
//! 进程清单决定；没有清单时 Windows 把 `comctl32.dll` 解析到 5.82，进程在加载期
//! 以 `STATUS_ENTRYPOINT_NOT_FOUND (0xc0000139)` 终止。
//! 应用 bin 由 `tauri build` 补清单，但**集成测试二进制没有任何清单** —— 于是
//! 任何触及 tauri 依赖链的集成测试都无法启动。这里统一给测试产物补上。
//!
//! 注意：**不能**用 `cargo:rustc-link-arg` —— 它同样作用于应用 bin，与 tauri 已
//! 嵌入的清单资源（RT_MANIFEST / ID 1）冲突，链接期报 `CVT1100 资源重复`。

fn main() {
    tauri_build::build();
    embed_common_controls_manifest_for_tests();
}

/// RT_MANIFEST 资源 ID（与 tauri / 常规工具链一致）
const MANIFEST_RESOURCE_ID: u32 = 1;
/// RT_MANIFEST 资源类型
const MANIFEST_RESOURCE_TYPE: u32 = 24;

/// 给测试 / 基准产物嵌入 Common Controls v6 清单（仅 Windows；缺 `rc.exe` 或
/// 缺 Windows 资源链接器时跳过并打印提示，不阻断构建）。
///
/// 只发 `cargo:rustc-link-arg-tests` / `-benches`：cargo 自行限定作用域，
/// 应用 bin 不受影响（tauri 的清单保持唯一）。
fn embed_common_controls_manifest_for_tests() {
    println!("cargo:rerun-if-changed=build.rs");
    if std::env::var("CARGO_CFG_WINDOWS").is_err() {
        return;
    }

    let Ok(out_dir) = std::env::var("OUT_DIR").map(std::path::PathBuf::from) else {
        return;
    };

    const MANIFEST: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<assembly xmlns="urn:schemas-microsoft-com:asm.v1" manifestVersion="1.0">
  <assemblyIdentity type="win32" name="dsh-launcher-tests" version="0.0.0.0" processorArchitecture="amd64"/>
  <description>dsh-launcher 测试产物</description>
  <dependency>
    <dependentAssembly>
      <assemblyIdentity type="win32" name="Microsoft.Windows.Common-Controls"
                        version="6.0.0.0" processorArchitecture="amd64"
                        publicKeyToken="6595b64144ccf1df" language="*"/>
    </dependentAssembly>
  </dependency>
</assembly>
"#;
    let manifest_path = out_dir.join("test-manifest.xml");
    if std::fs::write(&manifest_path, MANIFEST).is_err() {
        return;
    }

    // 路径里的反斜杠必须转义（`\\`）：`rc.exe` 的字符串字面量会做 C 风格转义，
    // 未转义的 `\t`（`...\target\...`）会被吃掉成制表符 → RC2135 file not found。
    let rc_path = out_dir.join("test-manifest.rc");
    let rc_body = format!(
        "{} {} \"{}\"\n",
        MANIFEST_RESOURCE_ID,
        MANIFEST_RESOURCE_TYPE,
        manifest_path.display().to_string().replace('\\', "\\\\")
    );
    if std::fs::write(&rc_path, rc_body).is_err() {
        return;
    }

    if let Some(rc_exe) = find_windows_sdk_tool("rc.exe") {
        let res_path = out_dir.join("test-manifest.res");
        let mut command = std::process::Command::new(&rc_exe);
        command
            .args(["/nologo", "/fo"])
            .arg(&res_path)
            .arg(&rc_path);
        match command.output() {
            Ok(output) if output.status.success() => {
                // 让链接器直接读 `.res`（rustc/link.exe 原生支持）
                println!("cargo:rustc-link-arg-tests={}", res_path.display());
                return;
            }
            Ok(output) => {
                println!(
                    "cargo:warning=rc.exe 执行失败，跳过测试清单嵌入: {}",
                    String::from_utf8_lossy(&output.stderr).trim()
                );
                return;
            }
            Err(error) => {
                println!("cargo:warning=调用 rc.exe 失败，跳过测试清单嵌入: {error}");
                return;
            }
        }
    }

    // 没有 rc.exe：退化为只用链接器内嵌清单（要求 link.exe 与 cvtres 同版本）
    match find_windows_sdk_tool("cvtres.exe") {
        Some(cvtres) => {
            println!("cargo:rustc-link-arg-tests=/MANIFEST:EMBED");
            println!(
                "cargo:rustc-link-arg-tests=/MANIFESTINPUT:{}",
                manifest_path.display()
            );
            println!("cargo:rustc-link-arg-tests=/MANIFESTUAC:NO");
            println!("cargo:rustc-link-arg-tests=/cvtres:{}", cvtres.display());
        }
        None => {
            println!(
                "cargo:warning=未找到 Windows SDK 的 rc.exe / cvtres.exe，\
                 跳过为测试二进制嵌入 Common Controls v6 清单（触及 tauri 依赖链的集成测试可能无法启动）"
            );
        }
    }
}

/// 在 Windows SDK 安装目录里找工具（取版本号最大的一套；仅 x64）。
fn find_windows_sdk_tool(tool: &str) -> Option<std::path::PathBuf> {
    // 允许显式覆盖（CI / 非标准安装位置）
    if let Ok(explicit) = std::env::var("DSH_RC_EXE") {
        let path = std::path::PathBuf::from(explicit);
        if path.is_file() {
            return Some(path);
        }
    }
    let roots = [
        r"C:\Program Files (x86)\Windows Kits\10\bin",
        r"C:\Program Files\Windows Kits\10\bin",
    ];
    let mut best: Option<(Vec<u32>, std::path::PathBuf)> = None;
    for root in roots {
        let Ok(entries) = std::fs::read_dir(root) else {
            continue;
        };
        for entry in entries.flatten() {
            let candidate = entry.path().join("x64").join(tool);
            if !candidate.is_file() {
                continue;
            }
            let version: Vec<u32> = entry
                .file_name()
                .to_string_lossy()
                .split('.')
                .filter_map(|part| part.parse::<u32>().ok())
                .collect();
            if best
                .as_ref()
                .map(|(known, _)| version > *known)
                .unwrap_or(true)
            {
                best = Some((version, candidate));
            }
        }
    }
    best.map(|(_, path)| path)
}
