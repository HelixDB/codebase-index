use clap::{Args, Parser, Subcommand, ValueEnum};

pub mod version {
    pub const VERSION: &str = env!("CARGO_PKG_VERSION");
    pub const NAME: &str = "Helix CLI";
    pub const AUTHORS: &str = "Helix Team";
}

use version::{AUTHORS, NAME, VERSION};

#[derive(Debug, Parser)]
#[clap(name = NAME, version = VERSION, author = AUTHORS)]
pub struct HelixCLI {
    #[clap(subcommand)]
    pub command: CommandType,
}

#[derive(Debug, Subcommand)]
pub enum CommandType {
    /// Demo a Helix project
    Demo,

    /// Open graph vis in default browser
    Visualize(VisualizeCommand),

    /// Deploy a Helix project
    Deploy(DeployCommand),

    /// Update the helix-cli and helix-db repo
    Update(UpdateCommand),

    /// Re-deploy a Helix project with new queries
    Redeploy(RedeployCommand),

    /// Compile a Helix project
    Compile(CompileCommand),

    // /// Configure Helix Credentials
    // Config(ConfigCommand),
    /// Lint a Helix project
    Check(LintCommand),

    /// Install the Helix repo
    Install(InstallCommand),

    /// Initialise a new Helix project
    Init(InitCommand),

    /// Test a Helix project
    Test(TestCommand),

    /// List running Helix instances
    Instances(InstancesCommand),

    /// Stop Helix instances
    Stop(StopCommand),

    /// Start a stopped Helix instance
    Start(StartCommand),

    /// Give an instance a short description
    Label(LabelCommand),

    /// Save an instnaces data.mdb file
    Save(SaveCommand),

    /// Delete an instance and all its data
    Delete(DeleteCommand),

    /// Get the current version of the cli and db
    Version(VersionCommand),

    /// Check login credentials or login with github
    Login,

    /// Remove login credentials
    Logout,
}

#[derive(Debug, Args)]
#[clap(name = "deploy", about = "Deploy a Helix project")]
pub struct DeployCommand {
    #[clap(short, long, help = "The path to the project")]
    pub path: Option<String>,

    #[clap(short, long, help = "The output path")]
    pub output: Option<String>,

    #[clap(long, help = "Port to run the instance on")]
    pub port: Option<u16>,
}

#[derive(Debug, Args)]
#[clap(name = "update", about = "Update helix-cli and helix-db")]
pub struct UpdateCommand {}

#[derive(Debug, Args)]
#[clap(
    name = "version",
    about = "Get the installed verison of helix-cli and helix-db"
)]
pub struct VersionCommand {}

#[derive(Debug, Args)]
#[clap(
    name = "redeploy",
    about = "Re-deploy a Helix project with new queries"
)]
pub struct RedeployCommand {
    #[clap(help = "Existing helix cluster ID")]
    pub cluster: String,

    #[clap(short, long, help = "The path to the project")]
    pub path: Option<String>,

    #[clap(short, long, help = "Remote cluster ID")]
    pub remote: bool,
}

#[derive(Debug, Args)]
#[clap(name = "compile", about = "Compile a Helix project")]
pub struct CompileCommand {
    #[clap(short, long, help = "The path to the project")]
    pub path: Option<String>,

    #[clap(short, long, help = "The output path")]
    pub output: Option<String>,

    #[clap(short, long, help = "The target language")]
    pub r#gen: Option<OutputLanguage>,
}

#[derive(Debug, Args)]
#[clap(name = "check", about = "Lint a Helix project")]
pub struct LintCommand {
    #[clap(short, long, help = "The path to the project")]
    pub path: Option<String>,
}

#[derive(Debug, Args)]
#[clap(name = "install", about = "Install the Helix repo")]
pub struct InstallCommand {
    #[clap(short, long, help = "Install HelixDb on a specific branch (considered unstable)")]
    pub branch: Option<String>,

    #[clap(short, long, help = "The path to the project")]
    pub path: Option<String>,
}

#[derive(Debug, Args)]
#[clap(name = "init", about = "Initialise a new Helix project")]
pub struct InitCommand {
    #[clap(short, long, help = "The path to the project")]
    pub path: Option<String>,
}

#[derive(Debug, Args)]
#[clap(name = "test", about = "Test a Helix project")]
pub struct TestCommand {
    #[clap(short, long, help = "The path to the project")]
    pub path: Option<String>,

    #[clap(short, long, help = "The test to run")]
    pub test: Option<String>,
}

#[derive(Debug, Args)]
#[clap(name = "instances", about = "List running Helix instances")]
pub struct InstancesCommand {}

#[derive(Debug, Args)]
#[clap(name = "stop", about = "Stop Helix instances")]
pub struct StopCommand {
    #[clap(long, help = "Stop all running instances")]
    pub all: bool,

    #[clap(short, long, help = "Instance ID to stop")]
    pub instance: Option<String>,
}

#[derive(Debug, Args)]
#[clap(name = "start", about = "Start a stopped Helix instance")]
pub struct StartCommand {
    #[clap(help = "Instance ID to Start")]
    pub instance: String,
}

#[derive(Debug, Args)]
#[clap(name = "visualize", about = "Visualize the Helix graph")]
pub struct VisualizeCommand {
    #[clap(help = "Id of instance to visualize")]
    pub instance: String,

    #[clap(short, long, help = "Give nodes a label based on a property")]
    pub node_prop: Option<String>,
}

#[derive(Debug, Args)]
#[clap(name = "label", about = "Give an instance a short description")]
pub struct LabelCommand {
    #[clap(help = "Instance ID to label")]
    pub instance: String,

    #[clap(help = "Short description to label")]
    pub label: String,
}

#[derive(Debug, Args)]
#[clap(name = "save", about = "Save an instances data.mdb file")]
pub struct SaveCommand {
    #[clap(help = "Instance ID to save")]
    pub instance: String,

    #[clap(help = "Where to save the file to")]
    pub output: Option<String>,
}

#[derive(Debug, Args)]
#[clap(name = "delete", about = "Delete an instance and its saved data")]
pub struct DeleteCommand {
    #[clap(help = "Instance ID to delete")]
    pub instance: String,
}


#[derive(Debug, Args)]
#[clap(name = "config", about = "Configure Helix Credentials")]
pub struct ConfigCommand {}

#[derive(Debug, Subcommand, Clone, ValueEnum)]
#[clap(name = "output")]
pub enum OutputLanguage {
    #[clap(name = "rust", alias = "rs")]
    Rust,
    #[clap(name = "typescript", alias = "ts")]
    TypeScript,
}

impl PartialEq for OutputLanguage {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (OutputLanguage::TypeScript, OutputLanguage::TypeScript) => true,
            (OutputLanguage::Rust, OutputLanguage::Rust) => true,
            _ => false,
        }
    }
}
