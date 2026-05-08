use alloy_primitives::{Address, LogData};
use clap::{Parser, Subcommand};
use iri_string::types::UriString;
use serde::{Deserialize, Serialize};
use std::{
    num::{NonZeroU32, NonZeroU64},
    path::PathBuf,
};
use utils::{
    config::{CliEnvExt, ConfigBuilder},
    serde::deserialize_vec_string,
};
use warpdrive_types::{
    AtProtoAction, ChainKey, ComponentDigest, Credential, ServiceStatus, Timestamp, WorkflowId,
};
use wasm_pkg_client::{PackageRef, Version};

use crate::config::Config;

#[derive(Parser)]
#[command(
    version,
    about = "WarpDrive",
    long_about = r#"
WarpDrive: Stellar's Gateway to Off-Chain Power.

Discover more at: https://www.warp-drive.xyz/
"#
)]
pub enum Command {
    /// Uploads a WASI component
    UploadComponent {
        /// Path to the WASI component
        component_path: String,

        #[clap(flatten)]
        args: CliArgs,
    },

    /// Deploys a service by loading its definition from a URL.
    ///
    /// The URL can be:
    /// http:// or https:// pointing to a JSON service definition
    /// or ipfs:// with a valid CID as the host (e.g., ipfs://bafybeigdyrzt5sfp7udm7hu76uh7y26nf3efuylqabf3oclgtqy55fbzdi)
    ///
    /// # Prerequisites
    ///
    /// - Core contracts must have been previously deployed via the CLI
    ///
    /// - Service components must already be uploaded
    ///
    /// - Vectrs must already be registered on the contracts
    DeployService {
        /// URI pointing to the JSON service definition
        #[clap(long)]
        service_uri: UriString,

        /// Boolean indicating whether to also set the URL on the service manager
        #[clap(long)]
        set_uri: bool,

        /// Additional CLI arguments for the deployment operation
        #[clap(flatten)]
        args: CliArgs,
    },

    /// Execute a component directly, without going through WarpDrive.
    /// Env vars starting with the "WARPDRIVE_ENV" prefix will be picked up by the component.
    Exec {
        /// Path to the WASI component
        /// The component must implement the trigger-world WIT
        #[clap(long)]
        component: String,

        #[clap(flatten)]
        args: CliArgs,

        /// The payload data.
        /// If preceded by a `@`, will be treated as a file path.
        /// If preceded by a `0x`, will be treated as hex-encoded.
        /// Otherwise will be treated as raw string bytes.
        #[clap(long)]
        input: String,

        /// Optional fuel limit for component execution
        #[clap(long)]
        fuel_limit: Option<u64>,

        /// Optional time limit (seconds) for component execution
        #[clap(long)]
        time_limit: Option<u64>,

        /// Component config in KEY=VALUE format: --config a=1 --config b=2
        #[clap(long)]
        config: Vec<String>,

        /// Optional path to save the execution output as JSON.
        /// Writes the WASM response as formatted JSON
        #[clap(long = "output", short = 'o')]
        output_file: Option<PathBuf>,

        /// Submit execution result to this chain via IWarpDriveServiceHandler
        #[clap(long, requires_all = &["submit_handler", "vectr_credential"])]
        submit_chain: Option<ChainKey>,

        /// Contract address of the IWarpDriveServiceHandler to submit results to
        #[clap(long, requires_all = &["submit_chain", "vectr_credential"])]
        submit_handler: Option<alloy_primitives::Address>,

        /// Vectr credential for envelope signing (required for submission)
        #[clap(long, requires_all = &["submit_chain", "submit_handler"])]
        vectr_credential: Option<Credential>,

        /// Vectr HD index for envelope signing
        #[clap(long, requires = "vectr_credential")]
        vectr_hd_index: Option<u32>,

        /// Simulate the transaction execution as actual TriggerData (JSON format)
        #[clap(long)]
        simulates_trigger: Option<TriggerKind>,
    },

    /// Service management commands
    Service {
        #[clap(subcommand)]
        command: ServiceCommand,

        /// Output file path
        #[clap(long, short, default_value = "./service.json")]
        file: PathBuf,

        #[clap(flatten)]
        args: CliArgs,
    },

    /// Execute aggregator components directly
    ExecAggregator {
        #[clap(flatten)]
        args: CliArgs,

        /// Path to the WASI aggregator component
        #[clap(long)]
        component: String,

        /// Optional path to the input JSON file (creates dummy input if not provided)
        #[clap(long)]
        input: Option<String>,

        /// Optional fuel limit for component execution
        #[clap(long)]
        fuel_limit: Option<u64>,

        /// Optional time limit (seconds) for component execution
        #[clap(long)]
        time_limit: Option<u64>,

        /// Configuration key-value pairs for the component in format 'key=value'.
        /// Example: --config chain=evm:31337 --config service_handler=0x1234...
        #[clap(long)]
        config: Option<Vec<String>>,

        /// Optional path to save the raw execution output payload
        /// Writes the aggregator result as JSON to file
        #[clap(long = "output", short = 'o')]
        output_file: Option<PathBuf>,
    },
}

/// Commands for managing services
#[derive(Debug, Subcommand, Clone, Serialize, Deserialize)]
pub enum ServiceCommand {
    /// Generates a new Service JSON.
    Init {
        /// The name of the service (required)
        #[clap(long)]
        name: String,
    },
    /// Workflow management commands
    Workflow {
        #[clap(subcommand)]
        command: WorkflowCommand,
    },
    /// Operations on service manager
    Manager {
        #[clap(subcommand)]
        command: ManagerCommand,
    },
    /// Update the service status to Active or Paused
    UpdateStatus {
        #[clap(long)]
        status: ServiceStatus,
    },
    /// Validates the service JSON
    Validate {},
}

/// Commands for managing components
#[derive(Debug, Subcommand, Clone, Serialize, Deserialize)]
pub enum ComponentCommand {
    /// Set a component using a url
    SetSourceUri {
        /// The url of the component to set
        #[clap(long)]
        uri: UriString,
    },
    /// Set a component using a digest
    SetSourceDigest {
        /// The digest of the component to set
        #[clap(long)]
        digest: ComponentDigest,
    },
    /// Set a component using a registry package reference
    SetSourceRegistry {
        /// Optional domain for the registry (omit for default)
        #[clap(long)]
        domain: Option<String>,

        /// The package reference (e.g., "namespace/name@version")
        #[clap(long)]
        package: PackageRef,

        /// Optional version to use (omit for latest)
        #[clap(long)]
        version: Option<Version>,
    },
    /// Manage permissions of a workflow component
    Permissions {
        /// HTTP hosts allowed for access:
        /// Use --http-hosts '' to disallow all hosts
        /// Use --http-hosts '*' to allow all hosts
        /// Use --http-hosts 'host1' --http-hosts 'host2' ... to allow specific hosts
        /// Omit to leave HTTP permissions unchanged
        #[clap(long)]
        http_hosts: Option<Vec<String>>,

        /// Enable file system access
        #[clap(long)]
        file_system: Option<bool>,
    },
    /// Manage the fuel limit of a workflow component
    FuelLimit {
        /// Fuel limit value (omit to use default)
        #[clap(long)]
        fuel: Option<u64>,
    },
    /// Set maximum execution time for a workflow component
    TimeLimit {
        /// Maximum execution time in seconds (omit to use default)
        #[clap(long)]
        seconds: Option<u64>,
    },
    /// Manage the workflow component config (KV)
    Config {
        /// Configuration key-value pairs in format 'key=value'
        /// Omit to clear all config values
        #[clap(long, conflicts_with = "config_file")]
        values: Option<Vec<String>>,

        /// Configuration as JSON file containing flat key-value pairs
        /// Example: --config-file config.json
        /// Cannot be used together with --values
        #[clap(long = "config-file", conflicts_with = "values")]
        config_file: Option<PathBuf>,
    },
    /// Manage the workflow component env
    Env {
        /// Env values staring with 'WARPDRIVE_ENV'
        #[clap(long)]
        values: Option<Vec<String>>,
    },
}

#[derive(Debug, Subcommand, Clone, Serialize, Deserialize)]
pub enum WorkflowCommand {
    /// Add a workflow to a service
    Add {
        /// The ID of the workflow (optional, autogenerated if not supplied)
        #[clap(long)]
        id: Option<WorkflowId>,
    },
    /// Delete a workflow from a service
    Delete {
        /// The ID of the workflow to delete
        #[clap(long)]
        id: WorkflowId,
    },
    /// Component management commands
    Component {
        /// The ID of the workflow to edit
        #[clap(long)]
        id: WorkflowId,
        #[clap(subcommand)]
        command: ComponentCommand,
    },
    /// Operations on workflow triggers
    Trigger {
        #[clap(long)]
        id: WorkflowId,
        #[clap(subcommand)]
        command: TriggerCommand,
    },
    /// Operations on workflow submit
    Submit {
        #[clap(long)]
        id: WorkflowId,
        #[clap(subcommand)]
        command: SubmitCommand,
    },
}

#[derive(Debug, Subcommand, Clone, Serialize, Deserialize)]
pub enum ManagerCommand {
    /// Sets an EVM service manager
    SetEvm {
        #[clap(long)]
        chain: ChainKey,
        #[clap(long)]
        address: alloy_primitives::Address,
    },
    /// Sets a Stellar service manager
    SetStellar {
        #[clap(long)]
        chain: ChainKey,
        /// Stellar contract ID (C-address) of the deployed handler
        #[clap(long)]
        address: String,
    },
}

#[derive(Debug, Subcommand, Clone, Serialize, Deserialize)]
pub enum SubmitCommand {
    /// Set an aggregator submit for a workflow
    SetAggregator {},
    /// Set the submit to None for a workflow
    SetNone {},
    /// Commands for the aggregator component
    Component {
        #[clap(subcommand)]
        component: ComponentCommand,
    },
}

#[derive(Debug, Subcommand, Clone, Serialize, Deserialize)]
pub enum TriggerCommand {
    /// Set a Cosmos contract event trigger for a workflow
    SetCosmos {
        /// The bech32 contract address (e.g., "cosmos1...")
        #[clap(long)]
        address: String,

        /// The chain (e.g., "cosmos:cosmoshub-4")
        #[clap(long)]
        chain: ChainKey,

        /// The event type to listen for
        #[clap(long)]
        event_type: String,
    },

    /// Set an EVM contract event trigger for a workflow
    SetEvm {
        /// The hexadecimal EVM address (e.g., "0x1234...")
        #[clap(long)]
        address: alloy_primitives::Address,

        /// The chain (e.g., "evm:1")
        #[clap(long)]
        chain: ChainKey,

        /// The event hash as a hex string (32 bytes)
        #[clap(long)]
        event_hash: String,
    },

    /// Set a Stellar contract event trigger for a workflow
    SetStellar {
        /// The Stellar contract ID (C-address)
        #[clap(long)]
        contract_id: String,

        /// The chain (e.g., "stellar:pubnet")
        #[clap(long)]
        chain: ChainKey,

        /// Topic segment patterns, repeated. Each is one of:
        ///   `string:<value>` — exact ScVal::String match
        ///   `symbol:<value>` — exact ScVal::Symbol match
        ///   `wildcard`       — single-position wildcard
        ///   `rest-wildcard`  — remaining-positions wildcard (must be last)
        /// Example: `--topic string:swap --topic wildcard`
        #[clap(long = "topic")]
        topics: Vec<String>,
    },

    /// Set a block interval trigger for a workflow
    SetBlockInterval {
        #[clap(long)]
        chain: ChainKey,
        #[clap(long)]
        n_blocks: NonZeroU32,

        /// Optional start block
        #[clap(long)]
        start_block: Option<NonZeroU64>,

        /// Optional end block height
        #[clap(long)]
        end_block: Option<NonZeroU64>,
    },

    /// Set a cron trigger for a workflow
    SetCron {
        /// A cron expression defining the schedule for execution.
        #[clap(long)]
        schedule: cron::Schedule,

        /// Optional start time (timestamp in nanoseconds)
        #[clap(long)]
        start_time: Option<Timestamp>,

        /// Optional end time (timestamp in nanoseconds)
        #[clap(long)]
        end_time: Option<Timestamp>,
    },

    /// Set an ATProto Jetstream event trigger for a workflow
    SetAtProtocol {
        /// Collection NSID to filter for (e.g., "app.bsky.feed.post")
        /// Supports wildcards with prefix matching (e.g., "app.bsky.feed.*")
        #[clap(long)]
        collection: String,

        /// Optional DID to filter for specific repositories
        /// If not provided, will match events from any repository
        #[clap(long)]
        repo_did: Option<String>,

        /// Action type to filter for (create, update, delete)
        /// If not provided, will match all action types
        #[clap(long)]
        action: Option<AtProtoAction>,
    },
}

impl Command {
    pub fn args(&self) -> CliArgs {
        let args = match self {
            Self::DeployService { args, .. } => args,
            Self::UploadComponent { args, .. } => args,
            Self::Exec { args, .. } => args,
            Self::Service { args, .. } => args,
            Self::ExecAggregator { args, .. } => args,
        };

        args.clone()
    }

    pub fn config(&self) -> Config {
        ConfigBuilder::new(self.args()).build().unwrap()
    }
}

/// This struct is used for both args and environment variables
/// the basic idea is that every env var can be overriden by a cli arg
/// and these override the config file
/// env vars follow the pattern of WARPDRIVE_CLI_{UPPERCASE_ARG_NAME}
#[derive(Clone, Debug, Parser, Serialize, Deserialize, Default)]
#[command(version, about, long_about = None)]
#[serde(default)]
pub struct CliArgs {
    /// The home directory of the application, where the cli.toml configuration file is stored
    /// if not provided here or in an env var, a series of default directories will be tried
    #[arg(long)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub home: Option<PathBuf>,

    /// The WarpDrive endpoint. Default is `http://127.0.0.1:8000`
    #[arg(long)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub warpdrive_endpoint: Option<PathBuf>,

    /// The path to an optional dotenv file to try and load
    /// if not set, will be the current working directory's .env
    #[arg(long)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dotenv: Option<PathBuf>,

    /// Log level
    /// Default is "info"
    #[arg(long)]
    #[serde(skip_serializing_if = "Vec::is_empty")]
    #[serde(deserialize_with = "deserialize_vec_string")]
    pub log_level: Vec<String>,

    /// The directory to store all internal data files
    /// Default is /var/warpdrive-cli
    #[arg(long)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<PathBuf>,

    /// EVM credential for signing transactions (can be a mnemonic or private key)
    /// Usually leave this as None and override in env
    #[arg(long)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub evm_credential: Option<Credential>,

    /// Cosmos mnemonic (usually leave this as None and override in env)
    #[arg(long)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cosmos_mnemonic: Option<Credential>,

    /// Save the deployment (default is true)
    #[arg(long)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub save_deployment: Option<bool>,

    /// Do not display the results
    #[arg(long)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub quiet_results: Option<bool>,

    /// Returns result as JSON
    #[arg(long)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub json: Option<bool>,

    /// The IPFS gateway URL used to access IPFS content over HTTP.
    #[arg(long)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ipfs_gateway: Option<String>,
}

impl CliEnvExt for CliArgs {
    const ENV_VAR_PREFIX: &'static str = "WARPDRIVE_CLI";
    const TOML_IDENTIFIER: &'static str = "cli";

    fn home_dir(&self) -> Option<PathBuf> {
        self.home.clone()
    }

    fn dotenv_path(&self) -> Option<PathBuf> {
        self.dotenv.clone()
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum TriggerKind {
    /// Cron trigger time (in nanos)
    Cron {
        trigger_time: u64,
    },
    EvmContractEvent {
        chain: ChainKey,
        contract_address: Address,
        log_data: LogData,
        block_number: u64,
    },
    BlockInterval {
        chain: ChainKey,
        block_height: u64,
    },
}

impl std::str::FromStr for TriggerKind {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        serde_json::from_str(s)
            .map_err(|e| anyhow::anyhow!("Failed to parse TriggerKind JSON: {}", e))
    }
}

#[cfg(test)]
mod tests;
