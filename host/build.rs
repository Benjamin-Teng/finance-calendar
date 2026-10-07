fn main() {
    // Static-link the VC++ runtime on MSVC targets so the release exe does not
    // depend on VCRUNTIME140.dll / MSVCP140.dll at load time.
    //
    // Mechanism (verified against tauri-build 2.7.0 source,
    // crates/tauri-build/src/lib.rs `should_static_link_vc_runtime` +
    // `try_build`): tauri-build reads `build.windows.staticVCRuntime` from
    // tauri.conf.json (see ../tauri.conf.json) and, when true on an
    // `msvc` target, calls its internal `static_vcruntime::build()` helper,
    // which emits `cargo:rustc-link-arg=/DEFAULTLIB:libcmt.lib` (and
    // `/NODEFAULTLIB:msvcrt.lib` etc.) so the CRT is linked statically. This
    // is the *current*, non-deprecated mechanism -- the `STATIC_VCRUNTIME`
    // env var documented in older guides still works but prints a deprecation
    // warning and is not needed here.
    //
    // Note: `WindowsBuildConfig::default()` already defaults
    // `static_vc_runtime` to `true`, so this would happen even without the
    // explicit config entry; it is kept explicit in tauri.conf.json for
    // documentation purposes.
    tauri_build::try_build(tauri_build::Attributes::new().windows_attributes(
        tauri_build::WindowsAttributes::new().window_icon_path("icons/icon.ico"),
    ))
    .expect("failed to run tauri-build");

    // dynamic-wallpaper task 5.2：`src/app_icon.rs` 以 `tauri::include_image!` 內嵌
    // icons/png/*.png；那個巨集只追蹤它寫進 OUT_DIR 的快取檔、不追蹤來源 PNG，故在這裡宣告，
    // PNG 一換就重跑建置腳本並重編本 crate。
    println!("cargo:rerun-if-changed=icons/png");

    // tauri-build（經 tauri-winres）只以 `cargo:rustc-link-arg-bins` 把 resource.lib
    // （含宣告 Common Controls v6 的 app manifest）連結到 bin；examples 沒有 manifest 時
    // 會載入 comctl32 v5，找不到 v6 才有的 `TaskDialogIndirect` 匯出，行程卡在載入期的
    // hard error 對話框（task 1.1 實測：System 記錄檔事件 26「無法找到程序輸入點
    // TaskDialogIndirect」）。這裡把同一份 resource.lib 也連結到 examples
    // （`examples/probe_wind.rs` 等探針）。
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        if let Ok(out_dir) = std::env::var("OUT_DIR") {
            let res = std::path::Path::new(&out_dir).join("resource.lib");
            if res.exists() {
                println!("cargo:rustc-link-arg-examples={}", res.display());
            }
        }
    }
}
