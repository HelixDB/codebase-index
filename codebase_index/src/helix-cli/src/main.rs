use crate::{
    args::{CommandType, HelixCLI, OutputLanguage},
    instance_manager::InstanceManager,
    types::*,
    utils::*,
};
use clap::Parser;
use helixdb::{helix_engine::graph_core::config::Config, utils::styled_string::StyledString};
use sonic_rs::json;
use spinners::{Spinner, Spinners};
use std::{
    fmt::Write,
    fs::{self, OpenOptions, read_to_string},
    io::{Write as iWrite},
    path::{Path, PathBuf},
    process::{Command, Stdio},
};
pub mod args;
mod instance_manager;
mod types;
mod utils;

#[tokio::main]
async fn main() {
    let args = HelixCLI::parse();

    check_helix_version().await;

    match args.command {
        CommandType::Demo => {
            println!("{}", "Demoing Helix".green().bold());

            // run helix install
            Command::new("helix").arg("install").output().unwrap();

            // run helix init
            let output = Command::new("helix")
                .arg("init")
                .arg("--path")
                .arg("demo")
                .output()
                .unwrap();
            println!("└── {}", String::from_utf8_lossy(&output.stdout));

            // write queries.hx
            let queries_path = PathBuf::from("demo").join("queries.hx");
            fs::write(
                queries_path,
                r#"
QUERY CreateUser(name: String, age: U32, email: String) =>
    user <- AddN<User>({name: name, age: age, email: email})
    RETURN user


QUERY CreateFollow(follower_id: ID, followed_id: ID) =>
    follower <- N<User>(follower_id)
    followed <- N<User>(followed_id)
    AddE<Follows>::From(follower)::To(followed) // don't need to specify the `since` property because it has a default value
    RETURN "success"


QUERY CreatePost(user_id: ID, content: String) =>
    user <- N<User>(user_id)
    post <- AddN<Post>({content: content})
    AddE<Created>::From(user)::To(post) // don't need to specify the `created_at` property because it has a default value
    RETURN post


QUERY GetUsers() =>
    users <- N<User>
    RETURN users

QUERY GetPosts() =>
    posts <- N<Post>
    RETURN posts

QUERY GetPostsByUser(user_id: ID) =>
    posts <- N<User>(user_id)::Out<Created>
    RETURN posts


QUERY GetFollowedUsers(user_id: ID) =>
    followed <- N<User>(user_id)::Out<Follows>
    RETURN followed

QUERY GetFollowedUsersPosts(user_id: ID) =>
    followers <- N<User>(user_id)::Out<Follows>
    posts <- followers::Out<Created>::RANGE(0, 40)
    RETURN posts::{
        post: _::{content},
        creatorID: _::In<Created>::ID,
    }
            "#,
            )
            .unwrap();

            // write schema.hx
            let schema_path = PathBuf::from("demo").join("schema.hx");
            fs::write(
                schema_path,
                r#"
N::User {
    name: String,
    age: U32,
    email: String,
    created_at: Date DEFAULT NOW,
    updated_at: Date DEFAULT NOW,
}

N::Post {
    content: String,
    created_at: Date DEFAULT NOW,
    updated_at: Date DEFAULT NOW,
}

E::Follows {
    From: User,
    To: User,
    Properties: {
        since: Date DEFAULT NOW,
    }
}

E::Created {
    From: User,
    To: Post,
    Properties: {
        created_at: Date DEFAULT NOW,
    }
}
            "#,
            )
            .unwrap();

            // write call.sh
            let call_path = PathBuf::from("demo").join("call.sh");
            fs::write(
                call_path,
                r#"
if [ "$1" = "setup" ]; then
    curl -X POST http://localhost:6969/CreateUser -H "Content-Type: application/json" -d '{"name": "John Doe", "age": 25, "email": "john.doe@example.com"}'
    curl -X POST http://localhost:6969/CreateUser -H "Content-Type: application/json" -d '{"name": "Jane Doe", "age": 26, "email": "jane.doe@example.com"}'
    curl -X POST http://localhost:6969/CreateFollow -H "Content-Type: application/json" -d '{"follower_id": "1", "followed_id": "2"}'
    curl -X POST http://localhost:6969/CreatePost -H "Content-Type: application/json" -d '{"user_id": "1", "content": "Hello, world!"}'
elif [ "$1" = "2" ]; then
    curl -X POST http://localhost:6969/GetUsers -H "Content-Type: application/json"
elif [ "$1" = "3" ]; then
    curl -X POST http://localhost:6969/GetPosts -H "Content-Type: application/json"
elif [ "$1" = "4" ]; then
    curl -X POST http://localhost:6969/GetPostsByUser -H "Content-Type: application/json" -d '{"user_id": "1"}'
elif [ "$1" = "5" ]; then
    curl -X POST http://localhost:6969/GetFollowedUsers -H "Content-Type: application/json" -d '{"user_id": "1"}'
elif [ "$1" = "6" ]; then
    curl -X POST http://localhost:6969/GetFollowedUsersPosts -H "Content-Type: application/json" -d '{"user_id": "1"}'
elif [ "$1" = "7" ]; then
    curl -X POST http://localhost:6969/find_user_posts_with_creator_details -H "Content-Type: application/json" -d '{"user_id": "1"}'
else
    echo "Please provide argument 1 to create users or 2 to create follow relationship"
    exit 1
fi
            "#,
            )
            .unwrap();

            // run helix deploy
            let output = Command::new("helix")
                .arg("deploy")
                .arg("--path")
                .arg("demo")
                .output()
                .unwrap();
            println!("└── {}", String::from_utf8_lossy(&output.stdout));

            // print "to see the results run `sh call.sh`" in all green bold
            println!("{}", "to see the results run `sh call.sh`".green().bold());
        }

        CommandType::Deploy(command) => {
            match Command::new("cargo").output() {
                Ok(_) => {}
                Err(_) => {
                    println!("{}", "Cargo is not installed".red().bold());
                    return;
                }
            }

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

            let path = get_cfg_deploy_path(command.path).unwrap();

            let output = match command.output {
                Some(output) => output,
                None => dirs::home_dir()
                    .map(|path| {
                        path.join(".helix/repo/helix-db/helix-container")
                            .to_string_lossy()
                            .into_owned()
                    })
                    .unwrap_or_else(|| "./.helix/repo/helix-db/helix-container".to_string()),
            };

            let start_port = match command.port {
                Some(port) => port,
                None => 6969,
            };

            let port = match find_available_port(start_port) {
                Some(port) => {
                    if port != start_port {
                        println!(
                            "{} {} {} {} {}",
                            "Port".yellow(),
                            start_port,
                            "is in use, using port".yellow(),
                            port,
                            "instead".yellow(),
                        );
                    }
                    port
                }
                None => {
                    println!(
                        "{} {}",
                        "No available ports found starting from".red().bold(),
                        start_port
                    );
                    return;
                }
            };

            let files = match check_and_read_files(&path) {
                Ok(files) if !files.is_empty() => files,
                Ok(_) => {
                    println!("{}", "No queries found, nothing to compile".red().bold());
                    return;
                }
                Err(e) => {
                    println!("{} {}", "Error:".red().bold(), e);
                    return;
                }
            };

            let mut sp = Spinner::new(Spinners::Dots9, "Compiling Helix queries".into());

            let num_files = files.len();

            let (code, analyzed_source) = match generate(&files) {
                Ok(code) => code,
                Err(e) => {
                    sp.stop_with_message(format!("{}", "Error compiling queries".red().bold()));
                    println!("└── {}", e);
                    return;
                }
            };

            sp.stop_with_message(format!(
                "{} {} {}",
                "Successfully compiled".green().bold(),
                num_files,
                "query files".green().bold()
            ));

            let cache_dir = PathBuf::from(&output);
            fs::create_dir_all(&cache_dir).unwrap();

            let file_path = PathBuf::from(&output).join("src/queries.rs");
            let mut generated_rust_code = String::new();

            match write!(&mut generated_rust_code, "{}", analyzed_source) {
                Ok(_) => {
                    println!("{}", "Successfully transpiled queries".green().bold());
                }
                Err(e) => {
                    println!("{}", "Failed to transpile queries".red().bold());
                    println!("└── {} {}", "Error:".red().bold(), e);
                    return;
                }
            }

            match fs::write(file_path, generated_rust_code) {
                Ok(_) => {
                    println!("{}", "Successfully wrote queries file".green().bold());
                }
                Err(e) => {
                    println!("{}", "Failed to write queries file".red().bold());
                    println!("└── {} {}", "Error:".red().bold(), e);
                    return;
                }
            }

            let mut sp = Spinner::new(Spinners::Dots9, "Building Helix".into());

            let config_path = PathBuf::from(&output).join("src/config.hx.json");
            fs::copy(PathBuf::from(path.clone() + "/config.hx.json"), config_path).unwrap();
            let schema_path = PathBuf::from(&output).join("src/schema.hx");
            fs::copy(PathBuf::from(path.clone() + "/schema.hx"), schema_path).unwrap();

            let mut runner = Command::new("cargo");
            runner
                .arg("check")
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .current_dir(PathBuf::from(&output));

            match runner.output() {
                Ok(_) => {}
                Err(e) => {
                    sp.stop_with_message(format!("{}", "Failed to check Rust code".red().bold()));
                    println!("└── {} {}", "Error:".red().bold(), e);
                    return;
                }
            }

            let mut runner = Command::new("cargo");
            runner
                .arg("build")
                .arg("--release")
                .current_dir(PathBuf::from(&output))
                .env("RUSTFLAGS", "-Awarnings");

            match runner.output() {
                Ok(output) => {
                    if output.status.success() {
                        sp.stop_with_message(format!(
                            "{}",
                            "Successfully built Helix".green().bold()
                        ));
                    } else {
                        sp.stop_with_message(format!("{}", "Failed to build Helix".red().bold()));
                        let stderr = String::from_utf8_lossy(&output.stderr);
                        if !stderr.is_empty() {
                            println!("└── {} {}", "Error:\n".red().bold(), stderr);
                        }
                        return;
                    }
                }
                Err(e) => {
                    sp.stop_with_message(format!("{}", "Failed to build Helix".red().bold()));
                    println!("└── {} {}", "Error:".red().bold(), e);
                    return;
                }
            }

            let mut sp = Spinner::new(Spinners::Dots9, "Starting Helix instance".into());

            let instance_manager = InstanceManager::new().unwrap();

            let binary_path = dirs::home_dir()
                .map(|path| path.join(".helix/repo/helix-db/target/release/helix-container"))
                .unwrap();

            let endpoints: Vec<String> =
                code.source.queries.iter().map(|q| q.name.clone()).collect();

            match instance_manager.init_start_instance(&binary_path, port, endpoints) {
                Ok(instance) => {
                    sp.stop_with_message(format!(
                        "{}",
                        "Successfully started Helix instance".green().bold()
                    ));
                    print_instnace(&instance);
                }
                Err(e) => {
                    sp.stop_with_message(format!(
                        "{}",
                        "Failed to start Helix instance".red().bold()
                    ));
                    println!("└── {} {}", "Error:".red().bold(), e);
                    return;
                }
            }
        }

        // TODO: assuming just local for now
        CommandType::Visualize(command) => {
            let instance_manager = InstanceManager::new().unwrap();
            let iid = &command.instance;

            match instance_manager.get_instance(iid) {
                Ok(Some(instance)) => {
                    println!("{}", "Helix instance found!".green().bold());
                    let port = instance.port;
                    let url = format!("http://localhost:{}/get/graphvis", port);

                    if webbrowser::open(&url).is_ok() {
                    } else {
                        println!(
                            "{} {}",
                            "Failed to open graph visualizer for instance".red().bold(),
                            iid.red().bold()
                        );
                        return;
                    }
                }
                Ok(None) => {
                    println!(
                        "{} {}",
                        "No Helix instance found with id".red().bold(),
                        iid.red().bold()
                    );
                    return;
                }
                Err(e) => {
                    println!("{} {}", "Error:".red().bold(), e);
                    return;
                }
            };
        }

        CommandType::Update(_) => {
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

            if !check_cargo_version() {
                match Command::new("rustup").arg("update").output() {
                    Ok(_) => println!("{}", "Updating cargo!".green().bold()),
                    Err(e) => println!("{} {}", "Error updating cargo!", e),
                }
            } else {
                println!("{}", "cargo up-to-date!".green().bold());
            }

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
                let mut runner = Command::new("git");
                runner.arg("reset");
                runner.arg("--hard");
                runner.current_dir(&repo_path);
                match runner.output() {
                    Ok(_) => {}
                    Err(e) => {
                        println!(
                            "{} {}",
                            "Error while reseting installed helix-db version:"
                                .red()
                                .bold(),
                            e
                        );
                        return;
                    }
                }

                let mut runner = Command::new("git");
                runner.arg("pull");
                runner.current_dir(&repo_path);
                match runner.output() {
                    Ok(_) => println!(
                        "{}",
                        "New helix-db version successfully pulled!".green().bold()
                    ),
                    Err(e) => {
                        println!(
                            "{} {}",
                            "Error while pulling new helix-db version:".red().bold(),
                            e
                        );
                        return;
                    }
                }

                match get_n_helix_cli() {
                    Ok(_) => println!(
                        "{}",
                        "New helix-cli version successfully installed!"
                            .green()
                            .bold()
                    ),
                    Err(e) => {
                        println!(
                            "{} {}",
                            "Error while installing new helix-cli version:".red().bold(),
                            e
                        );
                        return;
                    }
                }
            } else {
                println!("{}", "HelixDB is up to date!".green().bold());
            }
        }

        CommandType::Version(_) => {
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
                Version::parse(&format!("v{}", get_crate_version(repo_path).unwrap())).unwrap();

            println!(
                "{} {}, {} {}",
                "helix-cli version:", local_cli_version, "helix-db version:", local_db_version
            );
        }

        CommandType::Redeploy(command) => {
            match Command::new("cargo").output() {
                Ok(_) => {}
                Err(_) => {
                    println!("{}", "Cargo is not installed".red().bold());
                    return;
                }
            }

            // if remote flag `--remote` is provided, upload queries to remote db
            if command.remote {
                let mut sp = Spinner::new(Spinners::Dots9, "Uploading queries to remote db".into());

                let path = match get_cfg_deploy_path(command.path) {
                    Ok(path) => path,
                    Err(_e) => {
                        sp.stop_with_message(format!(
                            "{}",
                            "Error getting config path".red().bold()
                        ));
                        return;
                    }
                };
                let files = match check_and_read_files(&path) {
                    Ok(files) if !files.is_empty() => files,
                    Ok(_) => {
                        sp.stop_with_message(format!(
                            "{}",
                            "No queries found, nothing to compile".yellow().bold()
                        ));
                        return;
                    }
                    Err(_e) => {
                        sp.stop_with_message(format!("{}", "Error getting files".red().bold()));
                        return;
                    }
                };

                let content = match generate_content(&files) {
                    Ok(content) => content,
                    Err(e) => {
                        sp.stop_with_message(format!(
                            "{}",
                            "Error generating content".red().bold()
                        ));
                        println!("└── {}", e);
                        return;
                    }
                };

                // get config from ~/.helix/credentials
                let home_dir = std::env::var("HOME").unwrap_or("~/".to_string());
                let config_path = &format!("{}/.helix", home_dir);
                let config_path = Path::new(config_path);
                let config_path = config_path.join("credentials");
                if !config_path.exists() {
                    sp.stop_with_message(format!("{}", "No credentials found".yellow().bold()));
                    println!(
                        "{}",
                        "Please run `helix config` to set your credentials"
                            .yellow()
                            .bold()
                    );
                    return;
                }

                // TODO: probable could make this more secure
                // reads credentials from ~/.helix/credentials
                let config = fs::read_to_string(config_path).unwrap();
                let user_id = config
                    .split("helix_user_id=")
                    .nth(1)
                    .unwrap()
                    .split("\n")
                    .nth(0)
                    .unwrap();
                let user_key = config
                    .split("helix_user_key=")
                    .nth(1)
                    .unwrap()
                    .split("\n")
                    .nth(0)
                    .unwrap();

                // read config.hx.json
                let config = match Config::from_files(
                    PathBuf::from(path.clone()).join("config.hx.json"),
                    PathBuf::from(path.clone()).join("schema.hx"),
                ) {
                    Ok(config) => config,
                    Err(e) => {
                        println!("Error loading config: {}", e);
                        sp.stop_with_message(format!("{}", "Error loading config".red().bold()));
                        return;
                    }
                };

                // upload queries to central server
                let payload = json!({
                    "user_id": user_id,
                    "queries": content.files,
                    "cluster_id": command.cluster,
                    "version": "0.1.0",
                    "helix_config": config.to_json()
                });
                println!("{:#?}", payload);
                let client = reqwest::Client::new();
                println!("{}", user_key);
                println!("{}", &command.cluster);
                match client
                    .post("http://ec2-184-72-27-116.us-west-1.compute.amazonaws.com:3000/clusters/deploy-queries")
                    .header("x-api-key", user_key) // used to verify user
                    .header("x-cluster-id", &command.cluster) // used to verify instance with user
                    .header("Content-Type", "application/json")
                    .body(sonic_rs::to_string(&payload).unwrap())
                    .send()
                    .await
                {
                    Ok(response) => {
                        if response.status().is_success() {
                            sp.stop_with_message(format!(
                                "{}",
                                "Queries uploaded to remote db".green().bold()
                            ));
                        } else {
                            sp.stop_with_message(format!(
                                "{}",
                                "Error uploading queries to remote db".red().bold()
                            ));
                            println!("└── {}", response.text().await.unwrap());
                            return;
                        }
                    }
                    Err(e) => {
                        sp.stop_with_message(format!(
                            "{}",
                            "Error uploading queries to remote db".red().bold()
                        ));
                        println!("└── {}", e);
                        return;
                    }
                };
            } else {
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

                let instance_manager = InstanceManager::new().unwrap();
                let iid = &command.cluster;

                match instance_manager.get_instance(iid) {
                    Ok(Some(_)) => println!("{}", "Helix instance found!".green().bold()),
                    Ok(None) => {
                        println!(
                            "{} {}",
                            "No Helix instance found with id".red().bold(),
                            iid.red().bold()
                        );
                        return;
                    }
                    Err(e) => {
                        println!("{} {}", "Error:".red().bold(), e);
                        return;
                    }
                };

                let path = get_cfg_deploy_path(command.path).unwrap();

                let output = dirs::home_dir()
                    .map(|path| {
                        path.join(".helix/repo/helix-db/helix-container")
                            .to_string_lossy()
                            .into_owned()
                    })
                    .unwrap_or_else(|| "./.helix/repo/helix-db/helix-container".to_string());

                let files = match check_and_read_files(&path) {
                    Ok(files) if !files.is_empty() => files,
                    Ok(_) => {
                        println!("{}", "No queries found, nothing to compile".red().bold());
                        return;
                    }
                    Err(e) => {
                        println!("{} {}", "Error:".red().bold(), e);
                        return;
                    }
                };

                let mut sp = Spinner::new(Spinners::Dots9, "Compiling Helix queries".into());

                let num_files = files.len();

                let (code, analyzed_source) = match generate(&files) {
                    Ok(code) => code,
                    Err(e) => {
                        sp.stop_with_message(format!("{}", "Error compiling queries".red().bold()));
                        println!("└── {}", e);
                        return;
                    }
                };

                sp.stop_with_message(format!(
                    "{} {} {}",
                    "Successfully compiled".green().bold(),
                    num_files,
                    "query files".green().bold()
                ));

                let cache_dir = PathBuf::from(&output);
                fs::create_dir_all(&cache_dir).unwrap();

                let file_path = PathBuf::from(&output).join("src/queries.rs");
                let mut generated_rust_code = String::new();
                match write!(&mut generated_rust_code, "{}", analyzed_source) {
                    Ok(_) => {}
                    Err(e) => {
                        println!("{}", "Failed to write queries file".red().bold());
                        println!("└── {} {}", "Error:".red().bold(), e);
                        return;
                    }
                }
                match fs::write(file_path, generated_rust_code) {
                    Ok(_) => {
                        println!("{}", "Successfully wrote queries file".green().bold());
                    }
                    Err(e) => {
                        println!("{}", "Failed to write queries file".red().bold());
                        println!("└── {} {}", "Error:".red().bold(), e);
                        return;
                    }
                }

                let mut sp = Spinner::new(Spinners::Dots9, "Building Helix".into());

                // copy config.hx.json to ~/.helix/repo/helix-db/helix-container/config.hx.json
                let config_path = PathBuf::from(&output).join("src/config.hx.json");
                fs::copy(PathBuf::from(path + "/config.hx.json"), config_path).unwrap();

                // check rust code
                let mut runner = Command::new("cargo");
                runner
                    .arg("check")
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .current_dir(PathBuf::from(&output));

                match runner.output() {
                    Ok(_) => {}
                    Err(e) => {
                        sp.stop_with_message(format!(
                            "{}",
                            "Failed to check Rust code".red().bold()
                        ));
                        println!("└── {} {}", "Error:".red().bold(), e);
                        return;
                    }
                }

                let mut runner = Command::new("cargo");
                runner
                    .arg("build")
                    .arg("--release")
                    .current_dir(PathBuf::from(&output))
                    .env("RUSTFLAGS", "-Awarnings");

                match runner.output() {
                    Ok(output) => {
                        if output.status.success() {
                            sp.stop_with_message(format!(
                                "{}",
                                "Successfully built Helix".green().bold()
                            ));
                        } else {
                            sp.stop_with_message(format!(
                                "{}",
                                "Failed to build Helix".red().bold()
                            ));
                            let stderr = String::from_utf8_lossy(&output.stderr);
                            if !stderr.is_empty() {
                                println!("└── {} {}", "Error:\n".red().bold(), stderr);
                            }
                            return;
                        }
                    }
                    Err(e) => {
                        sp.stop_with_message(format!("{}", "Failed to build Helix".red().bold()));
                        println!("└── {} {}", "Error:".red().bold(), e);
                        return;
                    }
                }

                match instance_manager.stop_instance(iid) {
                    Ok(_) => {}
                    Err(e) => {
                        println!("{} {}", "Error while stopping instance:".red().bold(), e);
                        return;
                    }
                }

                let mut sp = Spinner::new(Spinners::Dots9, "Starting Helix instance".into());

                let binary_path = dirs::home_dir()
                    .map(|path| path.join(".helix/repo/helix-db/target/release/helix-container"))
                    .unwrap();

                let endpoints: Vec<String> =
                    code.source.queries.iter().map(|q| q.name.clone()).collect();

                let cached_binary = instance_manager.cache_dir.join(&iid);
                match fs::copy(binary_path, &cached_binary) {
                    Ok(_) => {}
                    Err(e) => {
                        println!("{} {}", "Error while copying binary:".red().bold(), e);
                        return;
                    }
                }

                match instance_manager.start_instance(iid, Some(endpoints)) {
                    Ok(instance) => {
                        sp.stop_with_message(format!(
                            "{}",
                            "Successfully started Helix instance".green().bold()
                        ));
                        print_instnace(&instance);
                    }
                    Err(e) => {
                        sp.stop_with_message(format!(
                            "{}",
                            "Failed to start Helix instance".red().bold()
                        ));
                        println!("└── {} {}", "Error:".red().bold(), e);
                        return;
                    }
                }
            }
        }

        CommandType::Instances(_) => {
            let instance_manager = InstanceManager::new().unwrap();
            match instance_manager.list_instances() {
                Ok(instances) => {
                    if instances.is_empty() {
                        println!("No running Helix instances");
                        return;
                    }
                    for instance in instances {
                        print_instnace(&instance);
                        println!();
                    }
                }
                Err(e) => {
                    println!("{} {}", "Failed to list instances:".red().bold(), e);
                }
            }
        }

        CommandType::Stop(command) => {
            let instance_manager = InstanceManager::new().unwrap();
            match instance_manager.list_instances() {
                Ok(instances) => {
                    if !instance_manager.running_instances().unwrap() {
                        println!("{}", "No running Helix instances".bold());
                        return;
                    }
                    if command.all {
                        println!("{}", "Stopping all running Helix instances".bold());
                        instances.iter().for_each(|instance| {
                            if instance.running {
                                match instance_manager.stop_instance(instance.id.as_str()) {
                                    Ok(_) => {
                                        println!(
                                            "└── {} {}",
                                            "ID:".yellow().bold(),
                                            instance.id.yellow().bold()
                                        );
                                    }
                                    Err(e) => {
                                        println!(
                                            "{} {}, {}",
                                            "Failed to stop instance:".red().bold(),
                                            instance.id.red().bold(),
                                            e,
                                        );
                                    }
                                }
                            }
                        });
                    } else if let Some(instance_id) = command.instance {
                        match instance_manager.stop_instance(&instance_id) {
                            Ok(false) => {
                                println!(
                                    "{} {}",
                                    "Instance is not running".yellow().bold(),
                                    instance_id
                                )
                            }
                            Ok(true) => {
                                println!("{} {}", "Stopped instance".green().bold(), instance_id)
                            }
                            Err(e) => println!("{} {}", "Failed to stop instance:".red().bold(), e),
                        }
                    } else {
                        println!(
                            "{}",
                            "Please specify --all or provide an instance ID\n"
                                .yellow()
                                .bold()
                        );
                        println!("Available instances (green=running, yellow=stopped): ");
                        for instance in instances {
                            print_instnace(&instance);
                        }
                    }
                }
                Err(e) => {
                    println!("{} {}", "Failed to find instances:".red().bold(), e);
                }
            }
        }

        CommandType::Start(command) => {
            let instance_manager = InstanceManager::new().unwrap();
            let mut sp = Spinner::new(Spinners::Dots9, "Starting Helix instance".into());

            match instance_manager.start_instance(&command.instance, None) {
                Ok(instance) => {
                    sp.stop_with_message(format!(
                        "{}",
                        "Successfully started Helix instance".green().bold()
                    ));
                    print_instnace(&instance);
                }
                Err(e) => {
                    sp.stop_with_message(format!("{}", "Failed to start instance".red().bold()));
                    println!("└── {} {}", "Error:".red().bold(), e);
                }
            }
        }

        CommandType::Label(command) => {
            let instance_manager = InstanceManager::new().unwrap();
            let instance_id = command.instance;
            let label = command.label;
            match instance_manager.set_label(&instance_id, &label) {
                Ok(false) => {
                    println!("{} {}", "Instance not found".red().bold(), instance_id)
                }
                Ok(true) => {
                    println!("{} {}", "Labeled".green().bold(), instance_id)
                }
                Err(e) => println!("{} {}", "Failed to stop instance:".red().bold(), e),
            }
        }

        CommandType::Compile(command) => {
            let path = if let Some(p) = &command.path {
                p
            } else {
                println!(
                    "{} '{}'",
                    "No path provided, defaulting to".yellow().bold(),
                    DB_DIR.yellow().bold()
                );
                DB_DIR
            };

            let output = match &command.output {
                Some(output) => output.to_owned(),
                None => ".".to_string(),
            };

            let mut sp = Spinner::new(Spinners::Dots9, "Compiling Helix queries".into());
            let files = match check_and_read_files(&path) {
                Ok(files) => files,
                Err(e) => {
                    sp.stop_with_message(format!("{}", "Failed to read files".red().bold()));
                    println!("└── {}", e);
                    return;
                }
            };

            if files.is_empty() {
                sp.stop_with_message(format!(
                    "{}",
                    "No queries found, nothing to compile".red().bold()
                ));
                return;
            }

            let (_, analyzed_source) = match generate(&files) {
                Ok((code, analyzed_source)) => (code, analyzed_source),
                Err(e) => {
                    sp.stop_with_message(format!("{}", e.to_string().red().bold()));
                    return;
                }
            };

            if let Some(OutputLanguage::TypeScript) = command.r#gen {
                match gen_typescript(&analyzed_source, &output) {
                    Ok(_) => {}
                    Err(e) => {
                        println!("{} {}", "Failed to write typescript types".red().bold(), e);
                        println!("└── {} {}", "Error:".red().bold(), e);
                        return;
                    }
                };
            }

            let file_path = PathBuf::from(&output).join("queries.rs");
            let mut generated_rust_code = String::new();
            match write!(&mut generated_rust_code, "{}", analyzed_source) {
                Ok(_) => {
                    println!("{}", "Successfully transpiled queries".green().bold());
                }
                Err(e) => {
                    println!("{}", "Failed to transpile queries".red().bold());
                    println!("└── {} {}", "Error:".red().bold(), e);
                    return;
                }
            }
            match fs::write(file_path, generated_rust_code) {
                Ok(_) => {
                    println!(
                        "{} {}",
                        "Successfully compiled queries to".green().bold(),
                        output
                    );
                }
                Err(e) => {
                    println!("{} {}", "Failed to write queries file".red().bold(), e);
                    println!("└── {} {}", "Error:".red().bold(), e);
                    return;
                }
            }
        }

        CommandType::Check(command) => {
            let path = if let Some(p) = &command.path {
                p
            } else {
                println!(
                    "{} '{}'",
                    "No path provided, defaulting to".yellow().bold(),
                    DB_DIR.yellow().bold()
                );
                DB_DIR
            };

            let mut sp = Spinner::new(Spinners::Dots9, "Checking Helix queries".into());

            let files = match check_and_read_files(&path) {
                Ok(files) => files,
                Err(e) => {
                    sp.stop_with_message(format!("{}", "Error checking files".red().bold()));
                    println!("└── {}", e);
                    return;
                }
            };

            if files.is_empty() {
                sp.stop_with_message(format!(
                    "{}",
                    "No queries found, nothing to compile".red().bold()
                ));
                return;
            }

            match generate(&files) {
                Ok(_) => {}
                Err(e) => {
                    sp.stop_with_message(format!("{}", "Failed to generate queries".red().bold()));
                    println!("└── {}", e);
                    return;
                }
            }

            sp.stop_with_message(format!(
                "{}",
                "Helix-QL schema and queries validated successfully with zero errors"
                    .green()
                    .bold()
            ));
        }

        CommandType::Install(command) => {
            match Command::new("cargo").output() {
                Ok(_) => {}
                Err(_) => {
                    println!("{}", "Cargo is not installed".red().bold());
                    return;
                }
            }

            match Command::new("git").arg("version").output() {
                Ok(_) => {}
                Err(_) => {
                    println!("{}", "Git is not installed".red().bold());
                    return;
                }
            }

            let repo_path = {
                // check if helix repo exists
                let home_dir = match command.path {
                    Some(path) => PathBuf::from(path),
                    None => match dirs::home_dir() {
                        Some(dir) => dir,
                        None => {
                            println!("{}", "Could not determine home directory".red().bold());
                            return;
                        }
                    },
                };
                home_dir.join(".helix/repo")
            };

            if repo_path.clone().join("helix-db").exists()
                && repo_path.clone().join("helix-db").is_dir()
            {
                println!(
                    "{} {}",
                    "Helix repo already exists at".yellow().bold(),
                    repo_path
                        .join("helix-db")
                        .display()
                        .to_string()
                        .yellow()
                        .bold(),
                );
                return;
            }

            // Create the directory structure if it doesn't exist
            match fs::create_dir_all(&repo_path) {
                Ok(_) => println!(
                    "{} {}",
                    "Created directory structure at".green().bold(),
                    repo_path.display()
                ),
                Err(e) => {
                    println!("{}", "Failed to create directory structure".red().bold());
                    println!("|");
                    println!("└── {}", e);
                    return;
                }
            }

            let mut runner = Command::new("git");
            runner.arg("clone");
            runner.arg("https://github.com/HelixDB/helix-db.git");
            if let Some(branch) = command.branch {
                runner.arg("--branch");
                runner.arg(branch);
            }
            runner.current_dir(&repo_path);

            match runner.output() {
                Ok(_) => {
                    let home_dir = dirs::home_dir().unwrap();
                    println!(
                        "{} {}",
                        "Helix repo installed at".green().bold(),
                        home_dir.join(".helix/repo/").to_string_lossy()
                    );
                    println!("|");
                    println!("└── To get started, begin writing helix queries in your project.");
                    println!("|");
                    println!(
                        "└── Then run `helix check --path <path-to-project>` to check your queries."
                    );
                    println!("|");
                    println!(
                        "└── Then run `helix deploy --path <path-to-project>` to build your queries."
                    );
                }
                Err(e) => {
                    println!("{}", "Failed to install Helix repo".red().bold());
                    println!("|");
                    println!("└── {}", e);
                    return;
                }
            }
        }

        CommandType::Test(_command) => {
            unimplemented!("helix test coming soon!");
        }

        CommandType::Init(command) => {
            println!("Initialising Helix project...");
            let path = match command.path {
                Some(path) => PathBuf::from(path),
                None => PathBuf::from(DB_DIR),
            };
            let path_str = path.to_str().unwrap();

            let _ = match check_and_read_files(path_str) {
                Ok(files) if !files.is_empty() => {
                    println!(
                        "{} {}",
                        "Queries already exist in".yellow().bold(),
                        path_str
                    );
                    return;
                }
                Ok(_) => {}
                Err(_) => {}
            };

            fs::create_dir_all(&path).unwrap();

            let schema_path = path.join("schema.hx");
            fs::write(&schema_path, DEFAULT_SCHEMA).unwrap();

            let main_path = path.join("queries.hx");
            fs::write(main_path, DEFAULT_QUERIES).unwrap();

            let config_path = path.join("config.hx.json");
            fs::write(config_path, Config::init_config()).unwrap();

            println!(
                "{} {}",
                "Helix project initialised at".green().bold(),
                path.display()
            );
        }

        CommandType::Save(command) => {
            let instance_manager = InstanceManager::new().unwrap();
            let iid = &command.instance;

            match instance_manager.get_instance(iid) {
                Ok(Some(_)) => println!("{}", "Helix instance found!".green().bold()),
                Ok(None) => {
                    println!(
                        "{} {}",
                        "No Helix instance found with id".red().bold(),
                        iid.red().bold()
                    );
                    return;
                }
                Err(e) => {
                    println!("{} {}", "Error:".red().bold(), e);
                    return;
                }
            }

            let output_path = match command.output {
                Some(output) => format!("{}helix_instance_{}", output, iid),
                None => format!("helix_instance_{}", iid),
            };
            let home_dir = std::env::var("HOME").expect("Failed to get HOME environment variable");
            let instance_path = format!("{}/.helix/cached_builds/data/{}/user", home_dir, iid);

            let mut runner = Command::new("cp");
            runner.arg("-r");
            runner.arg(instance_path);
            runner.arg(&output_path);

            match runner.output() {
                Ok(_) => println!(
                    "{} {}",
                    "Saved Helix instance to".green().bold(),
                    output_path.green().bold()
                ),
                Err(e) => println!("{} {}", "Error while copying:".red().bold(), e),
            }
        }

        CommandType::Delete(command) => {
            let instance_manager = InstanceManager::new().unwrap();
            let iid = &command.instance;

            match instance_manager.get_instance(iid) {
                Ok(Some(_)) => println!("{}", "Helix instance found!".green().bold()),
                Ok(None) => {
                    println!(
                        "{} {}",
                        "No Helix instance found with id".red().bold(),
                        iid.red().bold()
                    );
                    return;
                }
                Err(e) => {
                    println!("{} {}", "Error:".red().bold(), e);
                    return;
                }
            }

            match instance_manager.stop_instance(iid) {
                Ok(true) => println!(
                    "{} {}",
                    "Stopped instance".green().bold(),
                    iid.green().bold()
                ),
                Ok(false) => {}
                Err(e) => println!("{} {}", "Error while stopping instance".red().bold(), e),
            }

            let mut _del_prompt: bool = false;
            print!("Are you sure you want to delete the instance and its data? (y/n): ");
            std::io::stdout().flush().unwrap();
            let mut input = String::new();
            std::io::stdin().read_line(&mut input).unwrap();
            _del_prompt = input.trim().to_lowercase() == "y";

            if _del_prompt {
                match instance_manager.delete_instance(iid) {
                    Ok(_) => println!("{}", "Deleted Helix instance".green().bold()),
                    Err(e) => println!("{} {}", "Error while deleting instance".red().bold(), e),
                }

                let home_dir =
                    std::env::var("HOME").expect("Failed to get HOME environment variable");
                let instance_path = format!("{}/.helix/cached_builds/data/{}", home_dir, iid);
                let binary_path = format!("{}/.helix/cached_builds/{}", home_dir, iid);
                let log_path = format!("{}/.helix/logs/instance_{}.log", home_dir, iid);
                let error_log_path = format!("{}/.helix/logs/instance_{}_error.log", home_dir, iid);

                let mut runner = Command::new("rm");
                runner.arg("-r");
                runner.arg(instance_path);
                runner.arg(binary_path);
                runner.arg(log_path);
                runner.arg(error_log_path);

                match runner.output() {
                    Ok(_) => println!("{}", "Deleted Helix instance data".green().bold()),
                    Err(e) => println!("{} {}", "Error while deleting data:".red().bold(), e),
                }
            }
        }

        CommandType::Login => {
            let home_dir = std::env::var("HOME").unwrap_or("~/".to_string());
            let config_path = &format!("{}/.helix", home_dir);
            let config_path = Path::new(config_path);
            if !config_path.exists() {
                fs::create_dir_all(config_path).unwrap();
            }

            let cred_path = config_path.join("credentials");

            if let Ok(contents) = read_to_string(&cred_path)
                && let Some(_key) = parse_credentials(&contents)
            {
                println!(
                    "You have an existing key which may be valid, only continue if it doesn't work or you want to switch accounts. (Key checking is WIP)"
                );
            }

            let key = github_login().await.unwrap();
            println!("{}", "Successfully logged in!".green().bold());

            let mut cred_file = OpenOptions::new()
                .read(true)
                .write(true)
                .create(true)
                .truncate(true)
                .open(cred_path)
                .unwrap();

            if let Err(e) = cred_file.write_all(&format!("helix_user_key={key}").into_bytes()) {
                println!(
                    "Got error when writing key: {}\nYou're key is: {}",
                    e.to_string().red(),
                    key
                );
            }
        }

        CommandType::Logout => {
            let home_dir = std::env::var("HOME").unwrap_or("~/".to_string());
            let config_path = &format!("{}/.helix", home_dir);
            let config_path = Path::new(config_path);
            if !config_path.exists() {
                fs::create_dir_all(config_path).unwrap();
            }

            let cred_path = config_path.join("credentials");
            if cred_path.exists() {
                fs::remove_file(cred_path).unwrap()
            }
        }
    }
}
