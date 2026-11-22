use std::fs::{self, File};
use std::io::{self, Write};
use std::path::Path;
use std::process::ExitCode;

const USAGE: &str = "\
Pemakaian:
  dim parse <file.dim> [--compact]   cetak model JSON ke stdout
  dim check <file.dim>               validasi saja, tanpa output JSON
  dim fmt <file.dim> [--check | --write]
                                    cetak bentuk kanonik, periksa, atau tulis ke file

File yang di-`import` atau di-`amends` dibaca relatif terhadap file yang menulisnya.

Diagnostik ditulis ke stderr. Exit code: 0 sukses, 1 ada error, 2 salah pemakaian / file tidak terbaca.";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (cmd, rest) = match args.split_first() {
        Some((cmd, rest)) => (cmd.as_str(), rest),
        None => return usage(),
    };
    if cmd == "fmt" {
        return fmt(rest);
    }
    let compact = rest.iter().any(|a| a == "--compact");
    let files: Vec<&String> = rest.iter().filter(|a| !a.starts_with("--")).collect();
    let emit_json = match cmd {
        "parse" => true,
        "check" => false,
        "-h" | "--help" | "help" => {
            println!("dim: parser bahasa diagram dim v{}\n\n{USAGE}", dim::model::VERSION);
            return ExitCode::SUCCESS;
        }
        _ => return usage(),
    };
    let [path] = files.as_slice() else { return usage() };

    let text = match read(path) {
        Ok(text) => text,
        Err(code) => return code,
    };
    // Nama di diagnostik dan `span.file` relatif terhadap folder file utama.
    let entry = Path::new(path.as_str());
    let name = entry.file_name().and_then(|n| n.to_str()).unwrap_or(path);
    let root = entry.parent().map(Path::to_path_buf).unwrap_or_default();
    let out = dim::compile_with(name, &text, &dim::load::FsLoader { root });

    if !out.diagnostics.is_empty() {
        eprintln!("{}", out.render_diagnostics());
    }
    let Some(model) = out.model else {
        let n = out.diagnostics.iter().filter(|d| d.is_error()).count();
        eprintln!("gagal: {n} error");
        return ExitCode::from(1);
    };
    if emit_json {
        let json = if compact {
            serde_json::to_string(&model)
        } else {
            serde_json::to_string_pretty(&model)
        };
        println!("{}", json.expect("model selalu bisa diserialisasi"));
    } else {
        eprintln!(
            "ok: {} file, {} elemen, {} relasi, {} flow",
            out.files.len(),
            model.nodes.len(),
            model.edges.len(),
            model.flows.len()
        );
    }
    ExitCode::SUCCESS
}

fn read(path: &str) -> Result<String, ExitCode> {
    fs::read_to_string(path).map_err(|e| {
        eprintln!("error: tidak bisa membaca `{path}`: {e}");
        ExitCode::from(2)
    })
}

fn fmt(args: &[String]) -> ExitCode {
    let mut path = None;
    let mut mode = None;
    for arg in args {
        match arg.as_str() {
            "--check" | "--write" if mode.is_none() => mode = Some(arg.as_str()),
            name if !name.starts_with('-') && path.is_none() => path = Some(name),
            _ => return usage(),
        }
    }
    let Some(path) = path else { return usage() };
    let source = match read(path) {
        Ok(text) => text,
        Err(code) => return code,
    };
    let formatted = match dim::format::format(&source) {
        Ok(formatted) => formatted,
        Err(diagnostics) => {
            let file = dim::diag::SourceFile::new(path, source);
            for diagnostic in diagnostics {
                eprint!("{}", dim::diag::render(&diagnostic, &file));
            }
            return ExitCode::from(1);
        }
    };
    match mode {
        Some("--check") if source != formatted => {
            eprintln!("`{path}` perlu diformat");
            return ExitCode::from(1);
        }
        Some("--write") if source != formatted => {
            if let Err(e) = write_canonical(Path::new(path), &source, &formatted) {
                eprintln!("error: tidak bisa menulis `{path}`: {e}");
                return ExitCode::from(2);
            }
        }
        None => print!("{formatted}"),
        _ => {}
    }
    ExitCode::SUCCESS
}

fn write_canonical(path: &Path, original: &str, formatted: &str) -> io::Result<()> {
    let target = path.canonicalize()?;
    let mode = target.metadata()?.permissions();
    let directory = target.parent().ok_or(io::ErrorKind::InvalidInput)?;
    let (temp, mut file) = (0..)
        .find_map(|n| {
            let temp = directory.join(format!(".dim-fmt-{}-{n}", std::process::id()));
            match File::create_new(&temp) {
                Ok(file) => Some(Ok((temp, file))),
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => None,
                Err(e) => Some(Err(e)),
            }
        })
        .expect("temporary file sequence is unbounded")?;
    let result = (|| {
        file.write_all(formatted.as_bytes())?;
        file.set_permissions(mode)?;
        file.sync_all()?;
        drop(file);
        if fs::read_to_string(&target)? != original {
            return Err(io::Error::other("isi file berubah saat diformat"));
        }
        fs::rename(&temp, target)
    })();
    if result.is_err() {
        let _ = fs::remove_file(temp);
    }
    result
}

fn usage() -> ExitCode {
    eprintln!("dim: parser bahasa diagram dim v{}\n\n{USAGE}", dim::model::VERSION);
    ExitCode::from(2)
}
