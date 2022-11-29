use anyhow::{Context, Result, Ok, bail};
use std::{
    fs,
    path,
    io::{Read, Write, Seek, SeekFrom},
};

fn bump_file(version: &String, file_path: &String, build_metadata: &bool) -> Result<()> {
    let path = path::PathBuf::from(file_path);

    let mut file = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(&path)
        .context(format!("failed to open file {}", file_path))?;

    let mut contents = String::new();
    file.read_to_string(&mut contents)
        .context(format!("failed to read file {}", file_path))?;

    // Find the first semver version with optional pre and build metadata in file with regex
    let re = regex::Regex::new(
        r"(\d+\.\d+\.\d+)(?:-([0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*))?(?:\+([0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*))?"
    ).unwrap();

    // TODO: This will match the first valid semver version in the file which will be wrong if the version key comes
    // after any other valid semver version in the file. This is a limitation of the regex approach.
    let caps = re.captures(&contents).context(format!("failed to find version in file {}", file_path))?;

    // Final version with optional build metadata
    let final_version = match build_metadata {
        true => match caps.get(3) {
            Some(build) => format!("{}+{}", version, build.as_str().parse::<u32>().unwrap() + 1),
            None => format!("{}+{}", version, 1),
        },
        false => version.to_string(),
    };
        
    // Replace file version with the final version
    let new_contents = contents.replacen(&caps[0], &final_version, 1);

    // Write the new contents to the file
    file.seek(SeekFrom::Start(0))
        .context(format!("failed to seek to start of file {}", file_path))?;

    file.write_all(new_contents.as_bytes())
        .context(format!("failed to write to file {}", file_path))?;

    Ok(())
}

pub fn bump_cargo(version: &String, file_path: &String, build_metadata: &bool) -> Result<()> {
    bump_file(version, file_path, build_metadata)
}

pub fn bump_npm(version: &String, file_path: &String, build_metadata: &bool) -> Result<()> {
    bump_file(version, file_path, build_metadata)
}

pub fn bump_pub(version: &String, file_path: &String, build_metadata: &bool) -> Result<()> {
    bump_file(version, file_path, build_metadata)
}

pub fn bump_android(_: &String, _: &String) -> Result<()> {
    bail!("android bumping is not yet supported");
}

pub fn bump_ios(_: &String, _: &String) -> Result<()> {
    bail!("ios bumping is not yet supported");
}