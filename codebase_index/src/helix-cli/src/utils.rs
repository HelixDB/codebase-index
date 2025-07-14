use crate::{instance_manager::InstanceInfo, types::*};
use futures_util::StreamExt;
use helixdb::{
    helixc::{
        analyzer::analyzer::analyze,
        generator::{generator_types::Source as GeneratedSource, tsdisplay::ToTypeScript},
        parser::helix_parser::{Content, HelixParser, HxFile, Source},
    },
    utils::styled_string::StyledString,
};
use reqwest::Client;
use serde::Deserialize;
use serde_json::Value as JsonValue;
use std::{
    error::Error,
    fs::{self, DirEntry, File},
    io::{ErrorKind, Write},
    net::{SocketAddr, TcpListener},
    path::{Path, PathBuf},
    process::{Command, Stdio},
};
use tokio_tungstenite::{
    connect_async,
    tungstenite::{
        Message,
        protocol::{CloseFrame, frame::coding::CloseCode},
    },
};
use toml::Value;

pub const DB_DIR: &str = "helixdb-cfg/";

pub const DEFAULT_SCHEMA: &str = r#"// Start building your schema here.
//
// The schema is used to to ensure a level of type safety in your queries.
//
// The schema is made up of Node types, denoted by N::,
// and Edge types, denoted by E::
//
// Under the Node types you can define fields that
// will be stored in the database.
//
// Under the Edge types you can define what type of node
// the edge will connect to and from, and also the
// properties that you want to store on the edge.
//
// Example:
//
// N::User {
//     Name: String,
//     Label: String,
//     Age: Integer,
//     IsAdmin: Boolean,
// }
//
// E::Knows {
//     From: User,
//     To: User,
//     Properties: {
//         Since: Integer,
//     }
// }
//
// For more information on how to write queries,
// see the documentation at https://docs.helix-db.com
// or checkout our GitHub at https://github.com/HelixDB/helix-db

V::Embedding {
    vec: [F64]
}
"#;

pub const DEFAULT_QUERIES: &str = r#"// Start writing your queries here.
//
// You can use the schema to help you write your queries.
//
// Queries take the form:
//     QUERY {query name}({input name}: {input type}) =>
//         {variable} <- {traversal}
//         RETURN {variable}
//
// Example:
//     QUERY GetUserFriends(user_id: String) =>
//         friends <- N<User>(user_id)::Out<Knows>
//         RETURN friends
//
//
// For more information on how to write queries,
// see the documentation at https://docs.helix-db.com
// or checkout our GitHub at https://github.com/HelixDB/helix-db

QUERY hnswinsert(vector: [F64]) =>
    AddV<Embedding>(vector)
    RETURN "Success"

QUERY hnswsearch(query: [F64], k: I32) =>
    res <- SearchV<Embedding>(query, k)
    RETURN res
"#;

pub fn check_helix_installation() -> Result<PathBuf, String> {
    let home_dir = dirs::home_dir().ok_or("Could not determine home directory")?;
    let repo_path = home_dir.join(".helix/repo/helix-db");
    let container_path = repo_path.join("helix-container");
    let cargo_path = container_path.join("Cargo.toml");

    if !repo_path.exists()
        || !repo_path.is_dir()
        || !container_path.exists()
        || !container_path.is_dir()
        || !cargo_path.exists()
    {
        return Err("run `helix install` first.".to_string());
    }

    Ok(container_path)
}

pub fn get_cfg_deploy_path(cmd_path: Option<String>) -> Result<String, CliError> {
    if let Some(path) = cmd_path {
        return Ok(path);
    }

    let cwd = ".";
    let files = match check_and_read_files(cwd) {
        Ok(files) => files,
        Err(_) => {
            return Ok(DB_DIR.to_string());
        }
    };

    if !files.is_empty() {
        return Ok(cwd.to_string());
    }

    Ok(DB_DIR.to_string())
}

pub fn find_available_port(start_port: u16) -> Option<u16> {
    let mut port = start_port;
    while port < 65535 {
        let addr = format!("0.0.0.0:{}", port).parse::<SocketAddr>().unwrap();
        match TcpListener::bind(addr) {
            Ok(listener) => {
                drop(listener);
                let localhost = format!("127.0.0.1:{}", port).parse::<SocketAddr>().unwrap();
                match TcpListener::bind(localhost) {
                    Ok(local_listener) => {
                        drop(local_listener);
                        return Some(port);
                    }
                    Err(e) => {
                        if e.kind() != ErrorKind::AddrInUse {
                            return None;
                        }
                        port += 1;
                        continue;
                    }
                }
            }
            Err(e) => {
                if e.kind() != ErrorKind::AddrInUse {
                    return None;
                }
                port += 1;
                continue;
            }
        }
    }
    None
}

/// Checks if the path contains a schema.hx and config.hx.json file
/// Returns a vector of DirEntry objects for all .hx files in the path
pub fn check_and_read_files(path: &str) -> Result<Vec<DirEntry>, CliError> {
    if !fs::read_dir(&path)
        .map_err(CliError::Io)?
        .any(|file| file.unwrap().file_name() == "schema.hx")
    {
        return Err(CliError::from(format!(
            "{}",
            "No schema file found".red().bold()
        )));
    }

    if !fs::read_dir(&path)
        .map_err(CliError::Io)?
        .any(|file| file.unwrap().file_name() == "config.hx.json")
    {
        return Err(CliError::from(format!(
            "{}",
            "No config.hx.json file found".red().bold()
        )));
    }

    let files: Vec<DirEntry> = fs::read_dir(&path)?
        .filter_map(|entry| entry.ok())
        .filter(|file| file.file_name().to_string_lossy().ends_with(".hx"))
        .collect();

    // Check for query files (exclude schema.hx)
    let has_queries = files.iter().any(|file| file.file_name() != "schema.hx");
    if !has_queries {
        return Err(CliError::from(format!(
            "{}",
            "No query files (.hx) found".red().bold()
        )));
    }

    Ok(files)
}

/// Generates a Content object from a vector of DirEntry objects
/// Returns a Content object with the files and source
///
/// This essentially makes a full string of all of the files while having a separate vector of the individual files
///
/// This could be changed in the future but keeps the option open for being able to access the files separately or all at once
pub fn generate_content(files: &Vec<DirEntry>) -> Result<Content, CliError> {
    let files: Vec<HxFile> = files
        .iter()
        .map(|file| {
            let name = file.path().to_string_lossy().into_owned();
            let content = fs::read_to_string(file.path()).unwrap();
            HxFile { name, content }
        })
        .collect();

    let content = files
        .clone()
        .iter()
        .map(|file| file.content.clone())
        .collect::<Vec<String>>()
        .join("\n");

    Ok(Content {
        content,
        files,
        source: Source::default(),
    })
}

/// Uses the helix parser to parse the content into a Source object
fn parse_content(content: &Content) -> Result<Source, CliError> {
    let source = match HelixParser::parse_source(&content) {
        Ok(source) => source,
        Err(e) => {
            return Err(CliError::from(format!("{}", e)));
        }
    };

    Ok(source)
}

/// Runs the static analyzer on the parsed source to catch errors and generate diagnostics if any.
/// Otherwise returns the generated source object which is an IR used to transpile the queries to rust.
fn analyze_source(source: Source) -> Result<GeneratedSource, CliError> {
    let (diagnostics, source) = analyze(&source);
    if !diagnostics.is_empty() {
        for diag in diagnostics {
            let filepath = diag.filepath.clone().unwrap_or("queries.hx".to_string());
            println!("{}", diag.render(&source.src, &filepath));
        }
        return Err(CliError::CompileFailed);
    }

    Ok(source)
}

/// Generates a Content and GeneratedSource object from a vector of DirEntry objects
/// Returns a tuple of the Content and GeneratedSource objects
///
/// This function is the main entry point for generating the Content and GeneratedSource objects
///
/// It first generates the content from the files, then parses the content into a Source object, and then analyzes the source to catch errors and generate diagnostics if any.
pub fn generate(files: &Vec<DirEntry>) -> Result<(Content, GeneratedSource), CliError> {
    let mut content = generate_content(&files)?;
    content.source = parse_content(&content)?;
    let analyzed_source = analyze_source(content.source.clone())?;
    Ok((content, analyzed_source))
}

pub fn print_instnace(instance: &InstanceInfo) {
    let rg: bool = instance.running;
    println!(
        "{} {}{}",
        if rg {
            "Instance ID:".green().bold()
        } else {
            "Instance ID:".yellow().bold()
        },
        if rg {
            instance.id.green().bold()
        } else {
            instance.id.yellow().bold()
        },
        if rg {
            " (running)".to_string().green().bold()
        } else {
            " (not running)".to_string().yellow().bold()
        },
    );
    println!("└── Label: {}", instance.label.underline());
    println!("└── Port: {}", instance.port);
    println!("└── Available endpoints:");
    instance
        .available_endpoints
        .iter()
        .for_each(|ep| println!("    └── /{}", ep));
}

pub fn gen_typescript(source: &GeneratedSource, output_path: &str) -> Result<(), CliError> {
    let mut file = File::create(PathBuf::from(output_path).join("interface.d.ts"))?;

    for node in &source.nodes {
        write!(file, "{}", node.to_typescript())?;
    }
    for edge in &source.edges {
        write!(file, "{}", edge.to_typescript())?;
    }
    for vector in &source.vectors {
        write!(file, "{}", vector.to_typescript())?;
    }

    Ok(())
}

pub fn get_crate_version<P: AsRef<Path>>(path: P) -> Result<Version, String> {
    let cargo_toml_path = path.as_ref().join("Cargo.toml");
    if !cargo_toml_path.exists() {
        return Err("Not a Rust crate: Cargo.toml not found".to_string());
    }

    let contents = fs::read_to_string(&cargo_toml_path)
        .map_err(|e| format!("Failed to read Cargo.toml: {}", e))?;

    let parsed_toml = contents
        .parse::<Value>()
        .map_err(|e| format!("Failed to parse Cargo.toml: {}", e))?;

    let version = parsed_toml
        .get("package")
        .and_then(|pkg| pkg.get("version"))
        .and_then(|v| v.as_str())
        .ok_or("Version field not found in [package] section")?;

    let vers = Version::parse(version)?;
    Ok(vers)
}

pub async fn get_remote_helix_version() -> Result<Version, Box<dyn Error>> {
    let client = Client::new();

    let url = "https://api.github.com/repos/HelixDB/helix-db/releases/latest";

    let response = client
        .get(url)
        .header("User-Agent", "rust")
        .header("Accept", "application/vnd.github+json")
        .send()
        .await?
        .text()
        .await?;

    let json: JsonValue = serde_json::from_str(&response)?;
    let tag_name = json
        .get("tag_name")
        .and_then(|v| v.as_str())
        .ok_or("Failed to find tag_name in response")?
        .to_string();

    Ok(Version::parse(&tag_name)?)
}

pub fn get_n_helix_cli() -> Result<(), Box<dyn Error>> {
    // TODO: running this through rust doesn't identify GLIBC so has to compile from source
    let status = Command::new("sh")
        .arg("-c")
        .arg("curl -sSL 'https://install.helix-db.com' | bash")
        .env(
            "PATH",
            format!(
                "{}:{}",
                std::env::var("HOME")
                    .map(|h| format!("{}/.cargo/bin", h))
                    .unwrap_or_default(),
                std::env::var("PATH").unwrap_or_default()
            ),
        )
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .status()?;

    if !status.success() {
        return Err(format!("Command failed with status: {}", status).into());
    }

    Ok(())
}

// TODO:
// Spinner::new
// Spinner::stop_with_message
// Dots9 style

pub async fn github_login() -> Result<String, Box<dyn Error>> {
    // TODO: get control server
    let url = "ws://127.0.0.1:3000/login";
    let (mut ws_stream, _) = connect_async(url).await?;

    let init_msg: UserCodeMsg = match ws_stream.next().await {
        Some(Ok(Message::Text(payload))) => sonic_rs::from_str(&payload)?,
        Some(Ok(m)) => return Err(format!("Unexpected message: {m:?}").into()),
        Some(Err(e)) => return Err(e.into()),
        None => return Err("Connection Closed Unexpectedly".into()),
    };

    println!(
        "To Login please go \x1b]8;;{}\x1b\\here\x1b]8;;\x1b\\({}),\nand enter the code: {}",
        init_msg.verification_uri,
        init_msg.verification_uri,
        init_msg.user_code.bold()
    );

    let msg: ApiKeyMsg = match ws_stream.next().await {
        Some(Ok(Message::Text(payload))) => sonic_rs::from_str(&payload)?,
        Some(Ok(Message::Close(Some(CloseFrame {
            code: CloseCode::Error,
            reason,
        })))) => return Err(format!("Error: {reason}").into()),
        Some(Ok(m)) => return Err(format!("Unexpected message: {m:?}").into()),
        Some(Err(e)) => return Err(e.into()),
        None => return Err("Connection Closed Unexpectedly".into()),
    };

    Ok(msg.key)
}

#[derive(Deserialize)]
struct UserCodeMsg {
    user_code: String,
    verification_uri: String,
}

#[derive(Deserialize)]
struct ApiKeyMsg {
    key: String,
}

/// tries to parse a credential file, returning the key, if any
pub fn parse_credentials(creds: &String) -> Option<&str> {
    for line in creds.lines() {
        if let Some((key, value)) = line.split_once("=")
            && key.to_lowercase() == "helix_user_key"
        {
            return Some(value);
        }
    }
    None
}

pub async fn check_helix_version() {
    match check_helix_installation() {
        Ok(_) => {}
        Err(_) => {
            println!(
                "{}",
                "Helix is not installed. Please run `helix install` first."
                .red()
                .bold()
            );
            return;
        }
    };

    let repo_path = {
        let home_dir = match dirs::home_dir() {
            Some(dir) => dir,
            None => {
                println!("{}", "Could not determine home directory".red().bold());
                return;
            }
        };
        home_dir.join(".helix/repo/helix-db/helixdb")
    };

    let local_cli_version =
        Version::parse(&format!("v{}", env!("CARGO_PKG_VERSION"))).unwrap();
    let local_db_version =
        Version::parse(&format!("v{}", get_crate_version(&repo_path).unwrap())).unwrap();
    let remote_helix_version = get_remote_helix_version().await.unwrap();
    println!(
        "helix-cli version: {}, helix-db version: {}, remote helix version: {}",
        local_cli_version, local_db_version, remote_helix_version
    );

    if local_db_version < remote_helix_version || local_cli_version < remote_helix_version {
        println!("{} {} {} {}",
            "New HelixDB version is available!".yellow().bold(),
            "Run".yellow().bold(),
            "helix update".white().bold(),
            "to install the newest version!".yellow().bold(),
        );
    }
}

pub fn check_cargo_version() -> bool {
    match Command::new("cargo").arg("--version").output() {
        Ok(output) => {
            let version_str = String::from_utf8_lossy(&output.stdout);
            let version = version_str
                .split_whitespace()
                .nth(1)
                .unwrap_or("0.0.0")
                .split('-')
                .next()
                .unwrap_or("0.0.0");
            let parts: Vec<u32> = version
                .split('.')
                .filter_map(|s| s.parse().ok())
                .collect();
            if parts.len() >= 2 {
                parts[0] >= 1 && parts[1] >= 88
            } else {
                false
            }
        }
        Err(_) => false,
    }
}

