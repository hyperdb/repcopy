use std::collections::HashSet;
use std::ffi::OsString;
use std::fs;
use std::io;
use std::path::{Component, Path, PathBuf};
use std::process::{Command, ExitCode};

const USAGE: &str = "\
Usage: repcopy <source directory> <target directory> [options]

Copies files from <source directory> to <target directory>.
  - Files that already exist in the target are skipped.
  - Empty directories are not copied.
  - --exclude takes comma-separated directory names located directly under the source root.
  - The .git directory at the source root is always excluded.

Options:
  --exclude=<list>   Directories to exclude (root level only), comma separated
  --ignored-only     Copy only files ignored by git (.gitignore etc.). Requires git
  -n, --dry-run      Show what would be copied without copying anything
  -h, --help         Show this help";

struct Args {
    source: PathBuf,
    target: PathBuf,
    excludes: HashSet<OsString>,
    dry_run: bool,
    ignored_only: bool,
}

#[derive(Default)]
struct Stats {
    copied: u64,
    skipped: u64,
    errors: u64,
}

fn parse_args() -> Result<Args, String> {
    let mut positional: Vec<String> = Vec::new();
    let mut excludes: HashSet<OsString> = HashSet::new();
    excludes.insert(OsString::from(".git"));
    let mut dry_run = false;
    let mut ignored_only = false;

    let mut iter = std::env::args().skip(1);
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "-h" | "--help" => {
                println!("{USAGE}");
                std::process::exit(0);
            }
            "-n" | "--dry-run" => dry_run = true,
            "--ignored-only" => ignored_only = true,
            "--exclude" => {
                let value = iter
                    .next()
                    .ok_or_else(|| "--exclude requires a value".to_string())?;
                add_excludes(&mut excludes, &value);
            }
            _ => {
                if let Some(value) = arg.strip_prefix("--exclude=") {
                    add_excludes(&mut excludes, value);
                } else if arg.starts_with('-') {
                    return Err(format!("unknown option: {arg}"));
                } else {
                    positional.push(arg);
                }
            }
        }
    }

    if positional.len() != 2 {
        return Err("source and target directories are required".to_string());
    }
    let target = PathBuf::from(positional.pop().unwrap());
    let source = PathBuf::from(positional.pop().unwrap());
    Ok(Args {
        source,
        target,
        excludes,
        dry_run,
        ignored_only,
    })
}

fn add_excludes(excludes: &mut HashSet<OsString>, value: &str) {
    for name in value.split(',') {
        let name = name.trim().trim_matches(|c| c == '/' || c == '\\');
        if !name.is_empty() {
            excludes.insert(OsString::from(name));
        }
    }
}

fn main() -> ExitCode {
    let args = match parse_args() {
        Ok(a) => a,
        Err(e) => {
            eprintln!("error: {e}\n\n{USAGE}");
            return ExitCode::from(2);
        }
    };

    if !args.source.is_dir() {
        eprintln!(
            "error: source directory not found: {}",
            args.source.display()
        );
        return ExitCode::from(2);
    }
    if args.target.exists() && !args.target.is_dir() {
        eprintln!(
            "error: target exists but is not a directory: {}",
            args.target.display()
        );
        return ExitCode::from(2);
    }

    let (source, target) = match (
        std::path::absolute(&args.source),
        std::path::absolute(&args.target),
    ) {
        (Ok(s), Ok(t)) => (s, t),
        (Err(e), _) | (_, Err(e)) => {
            eprintln!("error: failed to resolve path: {e}");
            return ExitCode::from(2);
        }
    };
    if same_path(&source, &target) {
        eprintln!("error: source and target are the same directory");
        return ExitCode::from(2);
    }

    let mut stats = Stats::default();

    // Collect candidate files as paths relative to the source root.
    let files = if args.ignored_only {
        match list_git_ignored(&source, &args.excludes) {
            Ok(files) => files,
            Err(e) => {
                eprintln!("error: {e}");
                return ExitCode::from(2);
            }
        }
    } else {
        let mut files = Vec::new();
        walk_dir(&source, Path::new(""), &target, &args.excludes, &mut files, &mut stats);
        files
    };

    for rel in files {
        if is_excluded(&rel, &args.excludes) || is_inside(&source.join(&rel), &target) {
            continue;
        }
        copy_file(&source, &target, &rel, args.dry_run, &mut stats);
    }

    if args.dry_run {
        println!(
            "\nDry run: {} would be copied, {} skipped (already exist), {} errors",
            stats.copied, stats.skipped, stats.errors
        );
    } else {
        println!(
            "\nDone: {} copied, {} skipped (already exist), {} errors",
            stats.copied, stats.skipped, stats.errors
        );
    }
    if stats.errors > 0 {
        ExitCode::from(1)
    } else {
        ExitCode::SUCCESS
    }
}

fn same_path(a: &Path, b: &Path) -> bool {
    match (fs::canonicalize(a), fs::canonicalize(b)) {
        (Ok(a), Ok(b)) => a == b,
        _ => a == b,
    }
}

/// True if `path` is `dir` itself or lies under it (used to never copy the target into itself).
fn is_inside(path: &Path, dir: &Path) -> bool {
    // Both paths are absolute; a lexical check is enough and avoids a syscall per file.
    path.starts_with(dir)
}

/// True if the first component of `rel` is one of the root-level excluded directories.
fn is_excluded(rel: &Path, excludes: &HashSet<OsString>) -> bool {
    let mut comps = rel.components();
    match (comps.next(), comps.next()) {
        // Only a directory (something with children) at the root can be excluded.
        (Some(Component::Normal(first)), Some(_)) => excludes.contains(first),
        _ => false,
    }
}

/// Recursively collects regular files under `source_root/rel_dir`.
fn walk_dir(
    source_root: &Path,
    rel_dir: &Path,
    target_root: &Path,
    excludes: &HashSet<OsString>,
    out: &mut Vec<PathBuf>,
    stats: &mut Stats,
) {
    let dir = source_root.join(rel_dir);
    let entries = match fs::read_dir(&dir) {
        Ok(e) => e,
        Err(e) => {
            report_error(&dir, &e, stats);
            return;
        }
    };

    let mut entries: Vec<_> = entries.filter_map(|e| e.ok()).collect();
    entries.sort_by_key(|e| e.file_name());

    for entry in entries {
        let rel = rel_dir.join(entry.file_name());
        let file_type = match entry.file_type() {
            Ok(t) => t,
            Err(e) => {
                report_error(&entry.path(), &e, stats);
                continue;
            }
        };

        if file_type.is_symlink() {
            eprintln!("warn: symlink ignored: {}", rel.display());
        } else if file_type.is_dir() {
            // Prune excluded root directories and the target itself (avoids walking node_modules etc.).
            if rel_dir.as_os_str().is_empty() && excludes.contains(&entry.file_name()) {
                continue;
            }
            if same_path(&entry.path(), target_root) {
                continue;
            }
            walk_dir(source_root, &rel, target_root, excludes, out, stats);
        } else if file_type.is_file() {
            out.push(rel);
        }
    }
}

/// Lists files ignored by git under `source` (respects .gitignore, .git/info/exclude
/// and the global excludes file), as paths relative to `source`.
fn list_git_ignored(source: &Path, excludes: &HashSet<OsString>) -> Result<Vec<PathBuf>, String> {
    let mut cmd = Command::new("git");
    cmd.arg("-C")
        .arg(source)
        .args(["ls-files", "-z", "--others", "--ignored", "--exclude-standard", "--", "."]);
    // Let git prune excluded root directories instead of listing their contents.
    for name in excludes {
        let mut spec = OsString::from(":(exclude,literal)");
        spec.push(name);
        cmd.arg(spec);
    }

    let output = cmd
        .output()
        .map_err(|e| format!("failed to run git (is it installed?): {e}"))?;
    if !output.status.success() {
        return Err(format!(
            "git ls-files failed (is the source inside a git repository?): {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }

    let stdout = String::from_utf8(output.stdout)
        .map_err(|_| "git returned a non UTF-8 path".to_string())?;
    let mut files: Vec<PathBuf> = Vec::new();
    for entry in stdout.split('\0').filter(|s| !s.is_empty()) {
        // git always uses '/', rebuild with the platform separator.
        let rel: PathBuf = entry.split('/').collect();
        let full = source.join(&rel);
        match fs::symlink_metadata(&full) {
            Ok(m) if m.file_type().is_symlink() => {
                eprintln!("warn: symlink ignored: {}", rel.display());
            }
            Ok(m) if m.is_file() => files.push(rel),
            _ => {}
        }
    }
    files.sort();
    Ok(files)
}

fn copy_file(source_root: &Path, target_root: &Path, rel: &Path, dry_run: bool, stats: &mut Stats) {
    let src = source_root.join(rel);
    let dst = target_root.join(rel);

    // symlink_metadata: treat any existing entry (file, dir, link) as "exists".
    if fs::symlink_metadata(&dst).is_ok() {
        stats.skipped += 1;
        return;
    }
    if dry_run {
        stats.copied += 1;
        println!("would copy: {}", rel.display());
        return;
    }
    if let Some(parent) = dst.parent() {
        if let Err(e) = fs::create_dir_all(parent) {
            report_error(parent, &e, stats);
            return;
        }
    }
    match fs::copy(&src, &dst) {
        Ok(_) => {
            stats.copied += 1;
            println!("copied: {}", rel.display());
        }
        Err(e) => report_error(&src, &e, stats),
    }
}

fn report_error(path: &Path, err: &io::Error, stats: &mut Stats) {
    stats.errors += 1;
    eprintln!("error: {}: {err}", path.display());
}
