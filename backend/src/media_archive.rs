use anyhow::{Result, bail};
use sha2::{Digest, Sha256};
use std::{
    collections::HashSet,
    fs,
    io::{self, Read},
    path::{Component, Path, PathBuf},
    process::{Child, ChildStdout, Command, Stdio},
};

type Moves = Vec<(PathBuf, PathBuf)>;
fn non_empty_path(path: &Path) -> &Path {
    if path.as_os_str().is_empty() {
        Path::new(".")
    } else {
        path
    }
}
fn read_text(path: &Path) -> String {
    fs::read(path)
        .map(|bytes| {
            let text = String::from_utf8_lossy(&bytes);
            text.strip_prefix('\u{feff}').unwrap_or(&text).to_string()
        })
        .unwrap_or_default()
}

fn media_target(path: &Path) -> Option<(String, String)> {
    let content = read_text(path);
    let target = content
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty() && !line.starts_with('#'))?;
    let raw = url::Url::parse(target)
        .map(|url| url.path().to_string())
        .unwrap_or_else(|_| {
            target
                .split(['?', '#'])
                .next()
                .unwrap_or(target)
                .to_string()
        });
    let decoded = percent_encoding::percent_decode_str(&raw).decode_utf8_lossy();
    let filename = Path::new(decoded.as_ref()).file_name()?.to_str()?;
    let extension = format!(
        ".{}",
        Path::new(filename).extension()?.to_str()?.to_lowercase()
    );
    if ![
        ".avi", ".flv", ".iso", ".m2ts", ".mkv", ".mov", ".mp4", ".mpeg", ".mpg", ".rmvb", ".ts",
        ".webm", ".wmv",
    ]
    .contains(&extension.as_str())
    {
        return None;
    }
    let anchor = if extension == ".iso" {
        filename.to_string()
    } else {
        filename[..filename.len() - extension.len()].to_string()
    };
    Some((extension, anchor))
}
fn json_container(path: &Path) -> String {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(&read_text(path)) else {
        return String::new();
    };
    let value = value
        .as_array()
        .and_then(|array| array.first())
        .unwrap_or(&value);
    let source = value
        .get("MediaSourceInfo")
        .filter(|value| value.is_object())
        .unwrap_or(value);
    if source["MediaStreams"].as_array().is_none_or(Vec::is_empty) {
        return String::new();
    }
    source["Container"]
        .as_str()
        .filter(|value| !value.is_empty())
        .or_else(|| source["container"].as_str())
        .unwrap_or("")
        .trim()
        .to_lowercase()
}
fn container_matches(extension: &str, container: &str) -> bool {
    let aliases: &[&str] = match extension {
        ".mkv" => &["matroska", "mkv"],
        ".m2ts" => &["m2ts", "mpegts", "mpeg-ts", "ts"],
        ".ts" => &["mpegts", "mpeg-ts", "ts"],
        ".mov" => &["mov", "mp4", "quicktime"],
        ".mp4" => &["mp4"],
        ".mpeg" | ".mpg" => &["mpeg", "mpegvideo", "mpg"],
        ".rmvb" => &["realmedia", "rm", "rmvb"],
        ".webm" => &["matroska", "webm"],
        ".wmv" => &["asf", "wmv"],
        ".avi" => &["avi"],
        ".flv" => &["flv"],
        ".iso" => &["iso"],
        _ => &[],
    };
    container
        .split(|c: char| c == ',' || c == '/' || c.is_whitespace())
        .any(|token| aliases.contains(&token))
}
pub fn plan_restores(directory: &Path) -> Result<Moves> {
    let directory = non_empty_path(directory);
    let short = regex::Regex::new(r"(?i)^[A-Za-z0-9]{1,8}~[0-9A-Z](?:\.[A-Za-z0-9]{1,3})?$")?;
    let mut files = fs::read_dir(directory)?
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter(|path| {
            path.is_file()
                && short.is_match(path.file_name().unwrap_or_default().to_str().unwrap_or(""))
        })
        .collect::<Vec<_>>();
    files.sort_by_key(|path| {
        path.file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_lowercase()
    });
    let mut bundles = files
        .iter()
        .filter_map(|path| {
            media_target(path)
                .map(|(extension, anchor)| (path.clone(), extension, anchor, PathBuf::new()))
        })
        .collect::<Vec<_>>();
    if bundles.is_empty() {
        bail!("No short-name STRM files could be identified");
    }
    let json = files
        .iter()
        .map(|path| (path.clone(), json_container(path)))
        .filter(|(_, container)| !container.is_empty())
        .collect::<Vec<_>>();
    let nfos = files
        .iter()
        .filter(|path| {
            let text = read_text(path);
            let text = text.trim_start();
            text.starts_with("<?xml")
                && text
                    .chars()
                    .take(500)
                    .collect::<String>()
                    .to_lowercase()
                    .contains("<movie")
        })
        .cloned()
        .collect::<Vec<_>>();
    let mut used = bundles
        .iter()
        .map(|bundle| bundle.0.clone())
        .collect::<HashSet<_>>();
    for bundle in &mut bundles {
        let matches = json
            .iter()
            .filter(|(path, container)| {
                !used.contains(path) && container_matches(&bundle.1, container)
            })
            .collect::<Vec<_>>();
        if matches.len() != 1 {
            bail!(
                "Expected one {} media-info file, found {}",
                bundle.1,
                matches.len()
            );
        }
        bundle.3 = matches[0].0.clone();
        used.insert(bundle.3.clone());
    }
    let nfos = nfos
        .into_iter()
        .filter(|path| !used.contains(path))
        .collect::<Vec<_>>();
    if nfos.len() != bundles.len() {
        bail!("Short-name NFO files do not match the number of media versions");
    }
    let hashes = nfos
        .iter()
        .map(|path| fs::read(path).map(|bytes| Sha256::digest(bytes).to_vec()))
        .collect::<io::Result<HashSet<_>>>()?;
    if hashes.len() != 1 && bundles.len() > 1 {
        bail!("Multiple media versions have different NFO files; automatic pairing is unsafe");
    }
    bundles.sort_by(|a, b| a.2.cmp(&b.2));
    let mut moves = vec![];
    for (bundle, nfo) in bundles.into_iter().zip(nfos) {
        moves.extend([
            (bundle.0, directory.join(format!("{}.strm", bundle.2))),
            (
                bundle.3,
                directory.join(format!("{}-mediainfo.json", bundle.2)),
            ),
            (nfo, directory.join(format!("{}.nfo", bundle.2))),
        ]);
    }
    let targets = moves
        .iter()
        .map(|(_, target)| target)
        .collect::<HashSet<_>>();
    if targets.len() != moves.len() {
        bail!("Multiple short files resolve to the same target");
    }
    for (source, target) in &moves {
        if target.exists() && source != target {
            bail!("Target already exists: {}", target.display());
        }
    }
    Ok(moves)
}
pub fn apply_restores(moves: &[(PathBuf, PathBuf)]) -> Result<()> {
    let mut completed: Vec<&(PathBuf, PathBuf)> = vec![];
    for movement in moves {
        if let Err(error) = fs::rename(&movement.0, &movement.1) {
            for (source, target) in completed.into_iter().rev() {
                if target.exists() && !source.exists() {
                    fs::rename(target, source)?;
                }
            }
            return Err(error.into());
        }
        completed.push(movement);
    }
    Ok(())
}
pub fn pack_directory(source: &Path, archive: &Path) -> Result<PathBuf> {
    let source = fs::canonicalize(non_empty_path(source))?;
    if !source.is_dir() {
        bail!("Source is not a directory: {}", source.display());
    }
    let archive = if archive.is_absolute() {
        archive.to_path_buf()
    } else {
        std::env::current_dir()?.join(archive)
    };
    fs::create_dir_all(archive.parent().unwrap())?;
    let archive = if archive.exists() {
        fs::canonicalize(&archive)?
    } else {
        fs::canonicalize(archive.parent().unwrap())?.join(archive.file_name().unwrap())
    };
    let temporary = archive.with_file_name(format!(
        "{}.tmp",
        archive.file_name().unwrap().to_string_lossy()
    ));
    if temporary.exists() {
        fs::remove_file(&temporary)?;
    }
    let result = (|| -> Result<()> {
        let encoder = flate2::write::GzEncoder::new(
            fs::File::create(&temporary)?,
            flate2::Compression::default(),
        );
        let mut builder = tar::Builder::new(encoder);
        fn append(
            builder: &mut tar::Builder<flate2::write::GzEncoder<fs::File>>,
            path: &Path,
            name: &Path,
            temporary: &Path,
        ) -> Result<()> {
            if path == temporary {
                return Ok(());
            }
            if fs::symlink_metadata(path)?.is_dir() {
                builder.append_dir(name, path)?;
                let mut entries = fs::read_dir(path)?.collect::<io::Result<Vec<_>>>()?;
                entries.sort_by_key(|entry| entry.file_name());
                for entry in entries {
                    append(
                        builder,
                        &entry.path(),
                        &name.join(entry.file_name()),
                        temporary,
                    )?;
                }
            } else {
                builder.append_path_with_name(path, name)?;
            }
            Ok(())
        }
        append(
            &mut builder,
            &source,
            Path::new(source.file_name().unwrap()),
            &temporary,
        )?;
        builder.into_inner()?.finish()?;
        fs::rename(&temporary, &archive)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result?;
    Ok(archive)
}
struct Decompressor {
    child: Child,
    output: ChildStdout,
}
impl Read for Decompressor {
    fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
        if bytes.is_empty() {
            return Ok(0);
        }
        let count = self.output.read(bytes)?;
        if count == 0 && !self.child.wait()?.success() {
            return Err(io::Error::other("Archive decompression failed"));
        }
        Ok(count)
    }
}
impl Drop for Decompressor {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
fn reader(path: &Path) -> Result<Box<dyn Read>> {
    let mut magic = [0; 6];
    let count = fs::File::open(path)?.read(&mut magic)?;
    if count >= 2 && magic[..2] == [0x1f, 0x8b] {
        return Ok(Box::new(flate2::read::GzDecoder::new(fs::File::open(
            path,
        )?)));
    }
    let program = if count >= 3 && &magic[..3] == b"BZh" {
        Some("bzip2")
    } else if count == 6 && magic == [0xfd, b'7', b'z', b'X', b'Z', 0] {
        Some("xz")
    } else {
        None
    };
    if let Some(program) = program {
        let mut child = Command::new(program)
            .args(["-dc", "--"])
            .arg(path)
            .stdout(Stdio::piped())
            .spawn()?;
        let output = child.stdout.take().unwrap();
        return Ok(Box::new(Decompressor { child, output }));
    }
    Ok(Box::new(fs::File::open(path)?))
}
pub fn unpack_archive(
    archive: &Path,
    destination: &Path,
    delete_archive: bool,
) -> Result<Vec<PathBuf>> {
    let archive = fs::canonicalize(archive)?;
    let destination = non_empty_path(destination);
    fs::create_dir_all(destination)?;
    let destination = fs::canonicalize(destination)?;
    let mut members = vec![];
    let mut input = tar::Archive::new(reader(&archive)?);
    for entry in input.entries()? {
        let entry = entry?;
        let path = entry.path()?.into_owned();
        let kind = entry.header().entry_type();
        if path.is_absolute() || path.components().any(|part| part == Component::ParentDir) {
            bail!("Unsafe archive path: {}", path.display());
        }
        if kind.is_symlink()
            || kind.is_hard_link()
            || kind.is_block_special()
            || kind.is_character_special()
            || kind.is_fifo()
        {
            bail!("Unsupported archive member: {}", path.display());
        }
        members.push((path, kind.is_file() || kind.is_gnu_sparse()));
    }
    tar::Archive::new(reader(&archive)?).unpack(&destination)?;
    for (name, file) in &members {
        if *file && !destination.join(name).is_file() {
            bail!("Archive extraction verification failed: {}", name.display());
        }
    }
    let mut roots = members
        .iter()
        .filter_map(|(name, _)| {
            name.components()
                .find(|part| matches!(part, Component::Normal(_)))
                .map(|part| destination.join(part.as_os_str()))
        })
        .collect::<Vec<_>>();
    roots.sort();
    roots.dedup();
    for root in roots.into_iter().filter(|root| root.is_dir()) {
        match plan_restores(&root) {
            Ok(moves) => apply_restores(&moves)?,
            Err(error) if error.to_string() == "No short-name STRM files could be identified" => {}
            Err(error) => return Err(error),
        }
    }
    if delete_archive {
        fs::remove_file(archive)?;
    }
    Ok(members
        .into_iter()
        .map(|(name, _)| destination.join(name))
        .collect())
}
fn cli_options<'a>(
    args: &'a [String],
    flag: Option<&str>,
    usage: &str,
) -> Result<Option<(Vec<&'a String>, bool)>> {
    let mut positional = vec![];
    let (mut options, mut enabled, mut help, mut invalid) = (true, false, false, false);
    let negative_number = regex::Regex::new(r"^-\d+$|^-\d*\.\d+$")?;
    for arg in args {
        if options && arg == "--" {
            options = false;
        } else if options && (arg == "-h" || (arg.starts_with("--") && "--help".starts_with(arg))) {
            help = true;
        } else if options && flag.is_some_and(|flag| arg.starts_with("--") && flag.starts_with(arg))
        {
            enabled = true;
        } else if options && arg.starts_with('-') && arg != "-" && !negative_number.is_match(arg) {
            invalid = true;
        } else {
            positional.push(arg);
        }
    }
    if help {
        println!("{usage}");
        return Ok(None);
    }
    if invalid {
        bail!("{usage}");
    }
    Ok(Some((positional, enabled)))
}
pub fn run(args: &[String]) -> Result<()> {
    let usage = if args
        .first()
        .is_some_and(|arg| arg == "restore-media-short-names")
    {
        "Usage: restore-media-short-names DIRECTORY [--dry-run]"
    } else {
        "Usage: media-archive pack SOURCE ARCHIVE | unpack ARCHIVE DESTINATION [--keep-archive]"
    };
    match args.first().map(String::as_str) {
        Some("restore-media-short-names") => {
            let Some((positional, dry_run)) = cli_options(&args[1..], Some("--dry-run"), usage)?
            else {
                return Ok(());
            };
            if positional.len() != 1 {
                bail!("{usage}");
            }
            let moves = plan_restores(Path::new(positional[0]))?;
            for (source, target) in &moves {
                println!(
                    "{} -> {}",
                    source.file_name().unwrap().to_string_lossy(),
                    target.file_name().unwrap().to_string_lossy()
                );
            }
            if !dry_run {
                apply_restores(&moves)?;
            }
        }
        Some("media-archive") => {
            let command_index = args
                .iter()
                .skip(1)
                .position(|arg| arg == "--" || !arg.starts_with('-'))
                .map(|index| index + 1)
                .unwrap_or(args.len());
            let Some((top_level, _)) = cli_options(&args[1..command_index], None, usage)? else {
                return Ok(());
            };
            if !top_level.is_empty() {
                bail!("{usage}");
            }
            let command_index =
                command_index + usize::from(args.get(command_index).is_some_and(|arg| arg == "--"));
            let command = args.get(command_index).map(String::as_str);
            let flag = match command {
                Some("pack") => None,
                Some("unpack") => Some("--keep-archive"),
                _ => bail!("{usage}"),
            };
            let Some((positional, keep)) = cli_options(&args[command_index + 1..], flag, usage)?
            else {
                return Ok(());
            };
            if positional.len() != 2 {
                bail!("{usage}");
            }
            match command {
                Some("pack") => println!(
                    "{}",
                    pack_directory(Path::new(positional[0]), Path::new(positional[1]))?.display()
                ),
                Some("unpack") => {
                    unpack_archive(Path::new(positional[0]), Path::new(positional[1]), !keep)?;
                }
                _ => bail!("{usage}"),
            }
        }
        _ => bail!("{usage}"),
    }
    Ok(())
}
