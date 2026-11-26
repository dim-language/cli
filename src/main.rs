use std::collections::BTreeMap;
use std::fs::{self, File};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

use dim::model::{Model, SourceSpan};

const USAGE: &str = "\
Usage:
  dim check <file.dim> [--sources] [--root <id>=<folder>]...
                                    validate a model; prints nothing when it's valid
  dim parse <file.dim> [--compact] [--spans]
                                    print the JSON model to stdout
  dim fmt [<path>...] [--check | --write]
                                    print one file in canonical form, or check / write
                                    every `.dim` file and `dim.config` in a folder
  dim init [<folder>]               create a starting `dim.config` with a style for each container role

`dim.config` is read from the main file's folder. Files named in `import` or `amends` are read relative
to the file that names them.

`--sources` checks every `source` against a local checkout of its root. The checkout comes from
`--root`, or from the git repository the main file is in when its remote URL matches the root's `repo`.

Diagnostics go to stderr. Exit code: 0 success, 1 errors, 2 wrong usage or an unreadable file.";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some((cmd, rest)) = args.split_first() else {
        return usage();
    };
    match cmd.as_str() {
        "check" => check(rest),
        "parse" => parse(rest),
        "fmt" => fmt(rest),
        "init" => init(rest),
        "-h" | "--help" | "help" => {
            println!("{}", help());
            ExitCode::SUCCESS
        }
        _ => usage(),
    }
}

fn help() -> String {
    format!(
        "dim: the dim diagram language parser, v{}\n\n{USAGE}",
        dim::model::VERSION
    )
}

fn usage() -> ExitCode {
    eprintln!("{}", help());
    ExitCode::from(2)
}

fn read(path: &Path) -> Result<String, ExitCode> {
    fs::read_to_string(path).map_err(|e| {
        eprintln!("error: can't read `{}`: {e}", path.display());
        ExitCode::from(2)
    })
}

/// Compile proyek dengan file utama `path`. Nama di diagnostik dan `span.file` relatif terhadap folder file
/// utama. `Err` berisi exit code kalau file tidak terbaca atau model punya error.
fn compile(path: &str) -> Result<(Model, PathBuf), ExitCode> {
    let entry = Path::new(path);
    let text = read(entry)?;
    let name = entry.file_name().and_then(|n| n.to_str()).unwrap_or(path);
    let folder = entry.parent().map(Path::to_path_buf).unwrap_or_default();
    let out = dim::compile_with(name, &text, &dim::load::FsLoader { root: folder.clone() });
    if !out.diagnostics.is_empty() {
        eprintln!("{}", out.render_diagnostics());
    }
    match out.model {
        Some(model) => Ok((model, folder)),
        None => {
            let n = out.diagnostics.iter().filter(|d| d.is_error()).count();
            eprintln!("failed: {n} error{}", if n == 1 { "" } else { "s" });
            Err(ExitCode::from(1))
        }
    }
}

fn parse(args: &[String]) -> ExitCode {
    let (mut compact, mut spans, mut path) = (false, false, None);
    for arg in args {
        match arg.as_str() {
            "--compact" => compact = true,
            "--spans" => spans = true,
            name if !name.starts_with('-') && path.is_none() => path = Some(name),
            _ => return usage(),
        }
    }
    let Some(path) = path else { return usage() };
    let mut model = match compile(path) {
        Ok((model, _)) => model,
        Err(code) => return code,
    };
    if !spans {
        model.spans = None;
    }
    let json = if compact {
        serde_json::to_string(&model)
    } else {
        serde_json::to_string_pretty(&model)
    };
    println!("{}", json.expect("a model can always be serialized"));
    ExitCode::SUCCESS
}

fn check(args: &[String]) -> ExitCode {
    let (mut sources, mut roots, mut path) = (false, BTreeMap::new(), None);
    let mut args = args.iter();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--sources" => sources = true,
            "--root" => {
                let Some((id, folder)) = args.next().and_then(|v| v.split_once('=')) else {
                    return usage();
                };
                roots.insert(id.to_string(), PathBuf::from(folder));
            }
            name if !name.starts_with('-') && path.is_none() => path = Some(name),
            _ => return usage(),
        }
    }
    let Some(path) = path else { return usage() };
    let (model, folder) = match compile(path) {
        Ok(compiled) => compiled,
        Err(code) => return code,
    };
    if !sources {
        return ExitCode::SUCCESS;
    }
    let errors = check_sources(&model, &folder, &roots);
    for error in &errors {
        eprintln!("{error}");
    }
    if errors.is_empty() {
        ExitCode::SUCCESS
    } else {
        eprintln!(
            "failed: {} error{}",
            errors.len(),
            if errors.len() == 1 { "" } else { "s" }
        );
        ExitCode::from(1)
    }
}

// ---------------------------------------------------------------------------
// `dim check --sources` (SPEC §6.7)
// ---------------------------------------------------------------------------

/// Periksa setiap `source` terhadap checkout lokal root-nya: path harus ada, dan kalau ada `line`, file itu
/// minimal sepanjang itu.
fn check_sources(model: &Model, folder: &Path, given: &BTreeMap<String, PathBuf>) -> Vec<String> {
    let mut errors = Vec::new();
    for id in given.keys() {
        if !model.config.roots.iter().any(|r| &r.id == id) {
            errors.push(format!(
                "error: `--root {id}=...`: there's no root `{id}` in `dim.config`"
            ));
        }
    }
    let mut checkouts: BTreeMap<&str, Option<PathBuf>> = BTreeMap::new();
    for element in &model.elements {
        let Some(source) = &element.base().source else { continue };
        let at = model
            .spans
            .as_ref()
            .and_then(|s| s.elements.get(element.id()))
            .map(location)
            .unwrap_or_default();
        let checkout = checkouts.entry(source.root.as_str()).or_insert_with(|| {
            let root = model.config.roots.iter().find(|r| r.id == source.root)?;
            let found = given
                .get(&root.id)
                .cloned()
                .or_else(|| git_checkout(folder, &root.repo));
            if found.is_none() {
                errors.push(format!(
                    "error: root `{}` ({}) has no local checkout; add `--root {}=<folder>`",
                    root.id, root.repo, root.id
                ));
            }
            found
        });
        let Some(checkout) = checkout else { continue };
        let target = if source.path == "." {
            checkout.clone()
        } else {
            checkout.join(&source.path)
        };
        let shown = format!("{}:{}", source.root, source.path);
        let problem = match (fs::metadata(&target), source.line) {
            (Err(_), _) => Some(format!("`{shown}` doesn't exist in `{}`", checkout.display())),
            (Ok(meta), Some(_)) if meta.is_dir() => Some(format!("`{shown}` is a folder, but `line` is set")),
            (Ok(_), Some(line)) => match fs::read_to_string(&target) {
                Ok(text) if text.lines().count() < line as usize => Some(format!(
                    "`{shown}` has only {} lines, but `line` is {line}",
                    text.lines().count()
                )),
                Ok(_) => None,
                Err(e) => Some(format!("`{shown}` can't be read: {e}")),
            },
            (Ok(_), None) => None,
        };
        if let Some(problem) = problem {
            errors.push(format!("error: source `{}`: {problem}\n{at}", element.id()));
        }
    }
    errors
}

fn location(span: &SourceSpan) -> String {
    format!("  --> {}:{}:{}\n", span.file, span.line, span.col)
}

/// Folder teratas repo git tempat file utama berada, kalau salah satu remote-nya menunjuk ke `repo`.
fn git_checkout(folder: &Path, repo: &str) -> Option<PathBuf> {
    let folder = if folder.as_os_str().is_empty() {
        Path::new(".")
    } else {
        folder
    };
    let git = |args: &[&str]| {
        let out = Command::new("git").arg("-C").arg(folder).args(args).output().ok()?;
        out.status
            .success()
            .then(|| String::from_utf8_lossy(&out.stdout).into_owned())
    };
    let remotes = git(&["config", "--get-regexp", r"^remote\..*\.url$"])?;
    let wanted = repo_key(repo);
    remotes
        .lines()
        .filter_map(|line| line.split_once(' '))
        .any(|(_, url)| repo_key(url) == wanted)
        .then(|| git(&["rev-parse", "--show-toplevel"]))
        .flatten()
        .map(|top| PathBuf::from(top.trim()))
}

/// Bentuk pembanding URL repo: `https://github.com/acme/shop.git` dan `git@github.com:acme/shop` sama-sama
/// menjadi `github.com/acme/shop`.
fn repo_key(url: &str) -> String {
    let url = url.trim();
    let (rest, scp) = match url.split_once("://") {
        Some((_, rest)) => (rest, false),
        None => (url, true),
    };
    let rest = rest
        .split_once('@')
        .map_or(rest, |(user, host)| if user.contains('/') { rest } else { host });
    let rest = if scp {
        rest.replacen(':', "/", 1)
    } else {
        rest.to_string()
    };
    let rest = rest.trim_end_matches('/');
    let rest = rest.strip_suffix(".git").unwrap_or(rest);
    rest.to_lowercase()
}

// ---------------------------------------------------------------------------
// `dim fmt`
// ---------------------------------------------------------------------------

fn fmt(args: &[String]) -> ExitCode {
    let mut paths = Vec::new();
    let mut mode = None;
    for arg in args {
        match arg.as_str() {
            "--check" | "--write" if mode.is_none() => mode = Some(arg.as_str()),
            name if !name.starts_with('-') => paths.push(PathBuf::from(name)),
            _ => return usage(),
        }
    }
    if paths.is_empty() {
        paths.push(PathBuf::from("."));
    }
    let single = paths.len() == 1 && !paths[0].is_dir();
    let mut files = Vec::new();
    for path in &paths {
        if let Err(e) = collect(path, &mut files) {
            eprintln!("error: can't read `{}`: {e}", path.display());
            return ExitCode::from(2);
        }
    }
    let Some(mode) = mode else {
        if !single {
            eprintln!("error: for a folder or several files, use `--check` or `--write`");
            return ExitCode::from(2);
        }
        return match format_file(&files[0]) {
            Ok(Some((_, formatted))) => {
                print!("{formatted}");
                ExitCode::SUCCESS
            }
            Ok(None) => ExitCode::from(1),
            Err(code) => code,
        };
    };
    let mut failed = false;
    for file in &files {
        let (source, formatted) = match format_file(file) {
            Ok(Some(pair)) => pair,
            Ok(None) => {
                failed = true;
                continue;
            }
            Err(code) => return code,
        };
        if source == formatted {
            continue;
        }
        if mode == "--check" {
            eprintln!("`{}` needs formatting", file.display());
            failed = true;
        } else if let Err(e) = write_canonical(file, &source, &formatted) {
            eprintln!("error: can't write `{}`: {e}", file.display());
            return ExitCode::from(2);
        }
    }
    if failed { ExitCode::from(1) } else { ExitCode::SUCCESS }
}

/// File `.dim` dan `dim.config` di `path`, urut per nama. Folder tersembunyi dilewati.
fn collect(path: &Path, out: &mut Vec<PathBuf>) -> io::Result<()> {
    if !path.is_dir() {
        fs::metadata(path)?;
        out.push(path.to_path_buf());
        return Ok(());
    }
    let mut entries: Vec<PathBuf> = fs::read_dir(path)?
        .map(|e| e.map(|e| e.path()))
        .collect::<Result<_, _>>()?;
    entries.sort();
    for entry in entries {
        let name = entry.file_name().and_then(|n| n.to_str()).unwrap_or_default();
        if name.starts_with('.') {
            continue;
        }
        if entry.is_dir() {
            collect(&entry, out)?;
        } else if name.ends_with(".dim") || name == dim::load::CONFIG_FILE {
            out.push(entry);
        }
    }
    Ok(())
}

/// Isi file dan bentuk kanoniknya, atau `None` kalau ada error sintaks (sudah dicetak). `Err` kalau file tidak
/// terbaca.
fn format_file(path: &Path) -> Result<Option<(String, String)>, ExitCode> {
    let source = read(path)?;
    match dim::format::format(&source) {
        Ok(formatted) => Ok(Some((source, formatted))),
        Err(diagnostics) => {
            let file = dim::diag::SourceFile::new(path.display().to_string(), source);
            for diagnostic in diagnostics {
                eprint!("{}", dim::diag::render(&diagnostic, &file));
            }
            Ok(None)
        }
    }
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
            return Err(io::Error::other("the file changed while it was being formatted"));
        }
        fs::rename(&temp, target)
    })();
    if result.is_err() {
        let _ = fs::remove_file(temp);
    }
    result
}

// ---------------------------------------------------------------------------
// `dim init` (SPEC §5.6)
// ---------------------------------------------------------------------------

/// `dim.config` awal: satu style untuk setiap role container.
fn starter_config() -> String {
    const STYLES: &str = "\
style role app { icon browser; background blue; }
style role service { icon server; background accent; }
style role database { icon database; background neutral; }
style role queue { icon queue; background purple; }
style role storage { icon storage; background orange; }
style role cache { icon cache; background teal; }
";
    format!("dim \"{}\";\n\n{STYLES}", dim::model::VERSION)
}

fn init(args: &[String]) -> ExitCode {
    let folder = match args {
        [] => Path::new("."),
        [folder] if !folder.starts_with('-') => Path::new(folder.as_str()),
        _ => return usage(),
    };
    let target = folder.join(dim::load::CONFIG_FILE);
    let created =
        fs::create_dir_all(folder).and_then(|()| File::create_new(&target)?.write_all(starter_config().as_bytes()));
    match created {
        Ok(()) => {
            eprintln!("created: {}", target.display());
            ExitCode::SUCCESS
        }
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {
            eprintln!("error: `{}` already exists", target.display());
            ExitCode::from(1)
        }
        Err(e) => {
            eprintln!("error: can't write `{}`: {e}", target.display());
            ExitCode::from(2)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repo_urls_compare_across_forms() {
        let key = "github.com/acme/shop";
        for url in [
            "https://github.com/acme/shop",
            "https://github.com/acme/shop.git",
            "https://github.com/Acme/Shop/",
            "git@github.com:acme/shop.git",
            "ssh://git@github.com/acme/shop",
            "https://user@github.com/acme/shop",
        ] {
            assert_eq!(repo_key(url), key, "{url}");
        }
        assert_ne!(repo_key("https://github.com/acme/shop-web"), key);
    }

    #[test]
    fn starter_config_is_canonical_and_valid() {
        let config = starter_config();
        assert_eq!(dim::format::format(&config).unwrap(), config);
        let main = format!(
            "dim \"{}\";\n\nperson buyer \"Buyer\" {{ description \"Pembeli.\"; }}\n",
            dim::model::VERSION
        );
        let loader = dim::load::MemLoader::new([(dim::load::CONFIG_FILE, config)]);
        let out = dim::compile_with("main.dim", &main, &loader);
        assert!(out.model.is_some(), "{}", out.render_diagnostics());
        assert_eq!(out.model.unwrap().config.styles.len(), 6);
    }
}
