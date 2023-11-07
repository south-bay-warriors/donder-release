use anyhow::{Context, Result, Ok, bail};
use regex::Captures;
use std::{
    fs,
    path,
    io::{Read, Write, Seek, SeekFrom},
};
use serde_json::{Map, Value};

fn version_data<'t>(text: &'t str) -> Option<Captures<'t>> {
    // Find the first semver version with optional pre and build metadata in file with regex
    let re = regex::Regex::new(
        r"(\d+\.\d+\.\d+)(?:-([0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*))?(?:\+([0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*))?"
    ).unwrap();

    re.captures(&text)
}

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

    // TODO: This will match the first valid semver version in the file which will be wrong if the version key comes
    // after any other valid semver version in the file. This is a limitation of the regex approach.
    let caps = version_data(&contents)
        .context(format!("failed to find version in file {}", file_path))?;

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

fn read_json(file_path: &str) -> Result<Map<String, Value>> {
    let content = fs::read_to_string(file_path)?;
    let json: Map<String, Value> = serde_json::from_str(&content)?;
    Ok(json)
}

fn write_json(file_path: &str, json: &Map<String, Value>) -> Result<()> {
    let content = serde_json::to_string_pretty(json)?;
    let mut file = fs::File::create(file_path)?;
    file.write_all(content.as_bytes())?;
    Ok(())
}

// Replaces the version in the package.json file
pub fn bump_npm(version: &String, file_path: &String, build_metadata: &bool) -> Result<()> {
    // Read the package.json file
    let mut package_json = read_json(file_path)?;

    // Capture metadata from version
    let caps = version_data(&version)
        .context(format!("failed to find metadata in version {}", file_path))?;

    // Final version with optional build metadata
    let final_version = match build_metadata {
        true => match caps.get(3) {
            Some(build) => format!("{}+{}", version, build.as_str().parse::<u32>().unwrap() + 1),
            None => format!("{}+{}", version, 1),
        },
        false => version.to_string(),
    };

    // Update the version field
    package_json["version"] = serde_json::Value::String(final_version);

    // Write the updated package.json back to the file
    write_json(file_path, &package_json)
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