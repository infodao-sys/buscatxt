//! Incluye el icono y la informacion de version en el ejecutable de Windows.
//!
//! No usa dependencias: genera un archivo de recursos (.rc) y lo compila con
//! la herramienta del sistema: `rc.exe` (Windows SDK) con MSVC o `windres`
//! con MinGW. En Linux no hace nada, porque los ejecutables ELF no llevan
//! icono. Si la herramienta no esta, avisa y el programa se compila sin icono.

use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=assets/buscatxt.ico");
    if env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }

    let manifest = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let out = PathBuf::from(env::var("OUT_DIR").unwrap());
    let icon = manifest.join("assets").join("buscatxt.ico");
    let rc = out.join("buscatxt.rc");
    fs::write(&rc, resource_script(&icon)).expect("no se pudo escribir el .rc");

    let compiled = if env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc") {
        compile_msvc(&rc, &out)
    } else {
        compile_gnu(&rc, &out)
    };
    match compiled {
        Ok(object) => println!("cargo:rustc-link-arg-bins={}", object.display()),
        Err(reason) => println!("cargo:warning=el ejecutable se compila sin icono: {reason}"),
    }
}

fn resource_script(icon: &Path) -> String {
    let version = env::var("CARGO_PKG_VERSION").unwrap();
    let part = |name: &str| env::var(name).unwrap().parse::<u16>().unwrap_or(0);
    let (major, minor, patch) = (
        part("CARGO_PKG_VERSION_MAJOR"),
        part("CARGO_PKG_VERSION_MINOR"),
        part("CARGO_PKG_VERSION_PATCH"),
    );
    // En las cadenas de un .rc la barra invertida es caracter de escape.
    let icon = icon.display().to_string().replace('\\', "\\\\");
    format!(
        r#"1 ICON "{icon}"

1 VERSIONINFO
FILEVERSION {major},{minor},{patch},0
PRODUCTVERSION {major},{minor},{patch},0
FILEOS 0x40004
FILETYPE 0x1
BEGIN
  BLOCK "StringFileInfo"
  BEGIN
    BLOCK "0C0A04B0"
    BEGIN
      VALUE "FileDescription", "buscatxt - busca texto en archivos y documentos"
      VALUE "FileVersion", "{version}"
      VALUE "ProductName", "buscatxt"
      VALUE "ProductVersion", "{version}"
      VALUE "InternalName", "buscatxt"
      VALUE "OriginalFilename", "buscatxt.exe"
      VALUE "LegalCopyright", "Licencia MIT"
    END
  END
  BLOCK "VarFileInfo"
  BEGIN
    VALUE "Translation", 0x0C0A, 1200
  END
END
"#
    )
}

/// Compila con `rc.exe`. El enlazador de MSVC acepta el `.res` directamente.
fn compile_msvc(rc: &Path, out: &Path) -> Result<PathBuf, String> {
    let tool = find_rc_exe().ok_or("no se encontro rc.exe (Windows SDK)")?;
    let res = out.join("buscatxt.res");
    run(Command::new(&tool).arg("/nologo").arg("/fo").arg(&res).arg(rc))?;
    Ok(res)
}

/// Busca `rc.exe` en el PATH y, si no esta, en la version mas nueva del
/// Windows SDK (en una terminal normal no esta en el PATH).
fn find_rc_exe() -> Option<PathBuf> {
    if Command::new("rc.exe").arg("/?").output().is_ok() {
        return Some(PathBuf::from("rc.exe"));
    }
    let arch = match env::var("HOST").unwrap_or_default() {
        h if h.starts_with("aarch64") => "arm64",
        h if h.starts_with("i686") => "x86",
        _ => "x64",
    };
    let roots = [env::var("ProgramFiles(x86)").ok(), env::var("ProgramFiles").ok()];
    let mut found: Vec<PathBuf> = roots
        .iter()
        .flatten()
        .map(|root| Path::new(root).join("Windows Kits").join("10").join("bin"))
        .filter_map(|bin| fs::read_dir(bin).ok())
        .flatten()
        .flatten()
        .map(|entry| entry.path().join(arch).join("rc.exe"))
        .filter(|path| path.is_file())
        .collect();
    // Las carpetas se llaman como la version del SDK (10.0.22621.0).
    found.sort_by_key(|path| {
        let name = path.parent()?.parent()?.file_name()?.to_str()?.to_owned();
        Some(name.split('.').map(|n| n.parse::<u32>().unwrap_or(0)).collect::<Vec<_>>())
    });
    found.pop()
}

/// Compila con `windres` (MinGW), que produce un objeto COFF enlazable.
fn compile_gnu(rc: &Path, out: &Path) -> Result<PathBuf, String> {
    let target = env::var("TARGET").unwrap_or_default();
    let prefixed = format!("{}-w64-mingw32-windres", target.split('-').next().unwrap_or("x86_64"));
    let object = out.join("buscatxt-res.o");
    let mut last_error = String::from("no se encontro windres");
    for tool in [prefixed.as_str(), "windres"] {
        match run(Command::new(tool).arg("-O").arg("coff").arg("-i").arg(rc).arg("-o").arg(&object)) {
            Ok(()) => return Ok(object),
            Err(error) => last_error = error,
        }
    }
    Err(last_error)
}

fn run(command: &mut Command) -> Result<(), String> {
    let program = command.get_program().to_string_lossy().into_owned();
    let output = command.output().map_err(|e| format!("{program}: {e}"))?;
    if output.status.success() {
        Ok(())
    } else {
        Err(format!("{program} fallo: {}", String::from_utf8_lossy(&output.stderr).trim()))
    }
}
