use anyhow::{Context, Result, Ok, bail};
use regex::Captures;
use std::{
    fs,
    path,
    io::{Read, Write, Seek, SeekFrom},
};
use serde_json::{Map, Value};

/// Extracts version data from a given text using a regular expression.
///
/// ## Arguments
///
/// * `text` - A string slice that contains the text to extract version data from.
///
/// ## Returns
///
/// An optional `regex::Captures` struct that contains the captured version data.
/// 
/// ## Captures
/// 
/// * `1` - The semver version
/// * `2` - The pre-release version
/// * `3` - The build metadata
/// 
/// ## Example
/// 
/// ```
/// let text = "0.1.0-alpha.1+5";
/// let caps = version_data(text).unwrap();
/// 
/// assert_eq!(caps.get(1).unwrap().as_str(), "0.1.0");
/// assert_eq!(caps.get(2).unwrap().as_str(), "alpha.1");
/// assert_eq!(caps.get(3).unwrap().as_str(), "5");
/// ```
fn version_data<'t>(text: &'t str) -> Option<Captures<'t>> {
    let re = regex::Regex::new(
        r"(\d+\.\d+\.\d+)(?:-([0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*))?(?:\+([0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*))?"
    ).unwrap();

    re.captures(&text)
}

/// Parses the given path and file to return a result.
/// 
/// ## Arguments
/// 
/// * `path` - A reference to a string representing the path to parse.
/// * `file` - A string representing the file to parse.
/// 
/// ## Returns
/// 
/// A `Result` containing a string if parsing was successful, or an error if parsing failed.
/// 
/// ## Example
/// 
/// ```
/// let path = "<root>";
/// let file = "Cargo.toml";
/// 
/// let result = parse_path(&path, file.to_string());
/// 
/// assert_eq!(result.unwrap(), "Cargo.toml");
/// 
/// let path = "android";
/// let file = "app/build.gradle";
/// 
/// let result = parse_path(&path, file.to_string());
/// 
/// assert_eq!(result.unwrap(), "android/app/build.gradle");
/// ```
fn parse_path(path: &String, file: String) -> Result<String> {
    let path = path.replace("<root>", "");
    
    if path.is_empty() {
        Ok(file.to_string())
    } else {
        Ok(format!("{}/{}", path, file))
    }
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
    let p = parse_path(file_path, "Cargo.toml".to_string())?;
    bump_file(version, &p, build_metadata)
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

/// Bumps the version of a package.json file and writes the updated file back to disk.
///
/// ## Arguments
///
/// * `version` - A string slice that holds the new version to be set.
/// * `file_path` - A string slice that holds the path to the folder where to find package.json file.
/// * `build_metadata` - A boolean that indicates whether to include build metadata in the version.
///
/// ## Errors
///
/// This function can return an error if:
///
/// * The package.json file cannot be read or parsed.
/// * The version string does not contain valid metadata.
/// * The updated package.json file cannot be written back to disk.
///
/// ##xw Examples
///
/// ```
/// let version = "1.2.3".to_string();
/// let file_path = "<root>".to_string();
/// let build_metadata = true;
///
/// match bump_npm(&version, &file_path, &build_metadata) {
///     Ok(_) => println!("Package version updated successfully!"),
///     Err(e) => println!("Error: {}", e),
/// }
/// ```
pub fn bump_npm(version: &String, file_path: &String, build_metadata: &bool) -> Result<()> {
    // Read the package.json file
    let p = parse_path(file_path, "package.json".to_string())?;
    let mut package_json = read_json(&p)?;

    let pkg_version = package_json["version"].as_str().unwrap();

    // Capture metadata from version
    let caps = version_data(pkg_version)
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
    write_json(&p, &package_json)
}

pub fn bump_pub(version: &String, file_path: &String, build_metadata: &bool) -> Result<()> {
    let p = parse_path(file_path, "pubspec.yaml".to_string())?;
    bump_file(version, &p, build_metadata)
}

pub fn bump_android(_: &String, _: &String) -> Result<()> {
    bail!("android bumping is not yet supported");
}

pub fn bump_ios(version: &String, file_path: &String) -> Result<()> {
    // Capture version data from version
    let caps = version_data(&version)
        .context(format!("failed to find metadata in version {}", file_path))?;

    let marketing_version = caps.get(1).unwrap().as_str();
    let pre_release_version = match caps.get(2) {
        Some(pre_release) => pre_release.as_str(),
        None => "",
    };

    // App Store Connect is very limited in what it allows for version numbers. It only allows 3 period-separated
    // numbers, and the first number must be greater than 0. It also does not allow any pre-release or build metadata.
    // To account for this, we only support alpha, beta and rc pre-release ids, and translate them to numbers
    // from 1 to 3.
    // Any other pre-release ids will have a number of 4.
    // If no pre release id is provided, <pre_id>.<pre_id_number> will default to 5.0
    // which means it's not a pre release.
    let next_project_version = match pre_release_version {
        "" => "5.0".to_string(),
        _ => {
            let pre_release_components = pre_release_version.split(".").collect::<Vec<&str>>();
            let next_project_version_id = match pre_release_components[0] {
                "alpha" => 1,
                "beta" => 2,
                "rc" => 3,
                _ => 4,
            };

            format!("{}.{}", next_project_version_id, pre_release_components[1])
        }
    };

    // Get xcode project file path
    let p = format!("{}.xcodeproj/project.pbxproj", file_path.trim_end_matches("/"));

    // Read the xcode project file
    let mut xcode_project = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(&p)
        .context(format!("failed to open file {}", p))?;

    let mut contents = String::new();
    xcode_project.read_to_string(&mut contents)
        .context(format!("failed to read file {}", p))?;

    // Find the MARKETING_VERSION line
    let re = regex::Regex::new(r#"MARKETING_VERSION = .*;"#).unwrap();
    let caps = re.captures(&contents)
        .context(format!("failed to find MARKETING_VERSION in file {}", p))?;
    // Replace MARKETING_VERSION with the new version
    let new_contents = contents
        .replace(&caps[0], &format!("MARKETING_VERSION = {};", marketing_version));

    // Find the CURRENT_PROJECT_VERSION line
    let re = regex::Regex::new(r#"CURRENT_PROJECT_VERSION = .*;"#).unwrap();
    let caps = re.captures(&new_contents)
        .context(format!("failed to find CURRENT_PROJECT_VERSION in file {}", p))?;
    // Replace CURRENT_PROJECT_VERSION with the new version
    let new_contents = new_contents
        .replace(&caps[0], &format!("CURRENT_PROJECT_VERSION = {};", next_project_version));

    // Write the new contents to the file
    xcode_project.seek(SeekFrom::Start(0))
        .context(format!("failed to seek to start of file {}", p))?;

    xcode_project.write_all(new_contents.as_bytes())
        .context(format!("failed to write to file {}", p))?;

    Ok(())
}