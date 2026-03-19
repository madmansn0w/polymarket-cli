use std::str::FromStr;

use anyhow::{Context, Result, bail};
use clap::{Args, Subcommand};
use polymarket_client_sdk::auth::LocalSigner;
use polymarket_client_sdk::auth::Signer as _;
use polymarket_client_sdk::gamma::types::request::PublicProfileRequest;
use polymarket_client_sdk::{POLYGON, derive_proxy_wallet, derive_safe_wallet};

use crate::config;
use crate::output::OutputFormat;

#[derive(Args)]
pub struct WalletArgs {
    #[command(subcommand)]
    pub command: WalletCommand,
}

#[derive(Subcommand)]
pub enum WalletCommand {
    /// Generate a new random wallet and save to config
    Create {
        /// Overwrite existing wallet
        #[arg(long)]
        force: bool,
        /// Signature type: eoa, proxy (default), or gnosis-safe
        #[arg(long, default_value = "proxy")]
        signature_type: String,
    },
    /// Import an existing private key
    Import {
        /// Private key (hex, with or without 0x prefix)
        key: String,
        /// Overwrite existing wallet
        #[arg(long)]
        force: bool,
        /// Signature type: eoa, proxy (default), or gnosis-safe
        #[arg(long, default_value = "proxy")]
        signature_type: String,
    },
    /// Show the address of the configured wallet
    Address,
    /// Show wallet info (address, config path, key source)
    Show,
    /// Delete all config and keys (fresh install)
    Reset {
        /// Skip confirmation prompt
        #[arg(long)]
        force: bool,
    },
    /// Sync funder address from Polymarket profile (auto-resolve proxy wallet)
    Sync {
        /// EOA address to look up (defaults to configured wallet address)
        address: Option<polymarket_client_sdk::types::Address>,
    },
}

pub async fn execute(
    args: WalletArgs,
    output: OutputFormat,
    private_key_flag: Option<&str>,
    funder_flag: Option<&str>,
) -> Result<()> {
    match args.command {
        WalletCommand::Create {
            force,
            signature_type,
        } => cmd_create(output, force, &signature_type),
        WalletCommand::Import {
            key,
            force,
            signature_type,
        } => cmd_import(&key, output, force, &signature_type),
        WalletCommand::Address => cmd_address(output, private_key_flag),
        WalletCommand::Show => cmd_show(output, private_key_flag, funder_flag),
        WalletCommand::Reset { force } => cmd_reset(output, force),
        WalletCommand::Sync { address } => cmd_sync(output, private_key_flag, address).await,
    }
}

fn guard_overwrite(force: bool) -> Result<()> {
    if !force && config::config_exists() {
        bail!(
            "A wallet already exists at {}. Use --force to overwrite.",
            config::config_path()?.display()
        );
    }
    Ok(())
}

fn cmd_create(output: OutputFormat, force: bool, signature_type: &str) -> Result<()> {
    guard_overwrite(force)?;

    let signer = LocalSigner::random().with_chain_id(Some(POLYGON));
    let address = signer.address();
    let key_hex = format!("{:#x}", signer.to_bytes());

    config::save_wallet(&key_hex, POLYGON, signature_type)?;
    let config_path = config::config_path()?;
    let proxy_addr = derive_proxy_wallet(address, POLYGON);

    match output {
        OutputFormat::Json => {
            println!(
                "{}",
                serde_json::json!({
                    "address": address.to_string(),
                    "proxy_address": proxy_addr.map(|a| a.to_string()),
                    "signature_type": signature_type,
                    "config_path": config_path.display().to_string(),
                })
            );
        }
        OutputFormat::Table => {
            println!("Wallet created successfully!");
            println!("Address:        {address}");
            if let Some(proxy) = proxy_addr {
                println!("Proxy wallet:   {proxy}");
            }
            println!("Signature type: {signature_type}");
            println!("Config:         {}", config_path.display());
            println!();
            println!("IMPORTANT: Back up your private key from the config file.");
            println!("           If lost, your funds cannot be recovered.");
        }
    }
    Ok(())
}

fn cmd_import(key: &str, output: OutputFormat, force: bool, signature_type: &str) -> Result<()> {
    guard_overwrite(force)?;

    let signer = LocalSigner::from_str(key)
        .context("Invalid private key")?
        .with_chain_id(Some(POLYGON));
    let address = signer.address();
    let key_hex = format!("{:#x}", signer.to_bytes());

    config::save_wallet(&key_hex, POLYGON, signature_type)?;
    let config_path = config::config_path()?;
    let proxy_addr = derive_proxy_wallet(address, POLYGON);

    match output {
        OutputFormat::Json => {
            println!(
                "{}",
                serde_json::json!({
                    "address": address.to_string(),
                    "proxy_address": proxy_addr.map(|a| a.to_string()),
                    "signature_type": signature_type,
                    "config_path": config_path.display().to_string(),
                })
            );
        }
        OutputFormat::Table => {
            println!("Wallet imported successfully!");
            println!("Address:        {address}");
            if let Some(proxy) = proxy_addr {
                println!("Proxy wallet:   {proxy}");
            }
            println!("Signature type: {signature_type}");
            println!("Config:         {}", config_path.display());
        }
    }
    Ok(())
}

fn cmd_address(output: OutputFormat, private_key_flag: Option<&str>) -> Result<()> {
    let (key, _) = config::resolve_key(private_key_flag)?;
    let key = key.ok_or_else(|| anyhow::anyhow!("{}", config::NO_WALLET_MSG))?;

    let signer = LocalSigner::from_str(&key).context("Invalid private key")?;
    let address = signer.address();

    match output {
        OutputFormat::Json => {
            println!("{}", serde_json::json!({"address": address.to_string()}));
        }
        OutputFormat::Table => {
            println!("{address}");
        }
    }
    Ok(())
}

fn cmd_show(output: OutputFormat, private_key_flag: Option<&str>, funder_flag: Option<&str>) -> Result<()> {
    let (key, source) = config::resolve_key(private_key_flag)?;
    let signer = key.as_deref().and_then(|k| LocalSigner::from_str(k).ok());
    let address = signer.as_ref().map(|s| s.address().to_string());
    let proxy_addr = signer
        .as_ref()
        .and_then(|s| derive_proxy_wallet(s.address(), POLYGON))
        .map(|a| a.to_string());
    let safe_addr = signer
        .as_ref()
        .and_then(|s| derive_safe_wallet(s.address(), POLYGON))
        .map(|a| a.to_string());
    let funder = config::resolve_funder(funder_flag)?;

    let sig_type = config::resolve_signature_type(None)?;
    let config_path = config::config_path()?;

    match output {
        OutputFormat::Json => {
            println!(
                "{}",
                serde_json::json!({
                    "address": address,
                    "proxy_address": proxy_addr,
                    "safe_address": safe_addr,
                    "funder": funder,
                    "signature_type": sig_type,
                    "config_path": config_path.display().to_string(),
                    "source": source.label(),
                    "configured": address.is_some(),
                })
            );
        }
        OutputFormat::Table => {
            match &address {
                Some(addr) => println!("Address:        {addr}"),
                None => println!("Address:        (not configured)"),
            }
            if let Some(proxy) = &proxy_addr {
                println!("Proxy (derive): {proxy}");
            }
            if let Some(safe) = &safe_addr {
                println!("Safe (derive):  {safe}");
            }
            if let Some(ref f) = funder {
                println!("Funder:         {f}");
            }
            println!("Signature type: {sig_type}");
            println!("Config path:    {}", config_path.display());
            println!("Key source:     {}", source.label());
        }
    }
    Ok(())
}

fn cmd_reset(output: OutputFormat, force: bool) -> Result<()> {
    if !config::config_exists() {
        match output {
            OutputFormat::Table => println!("Nothing to reset. No config found."),
            OutputFormat::Json => {
                println!(
                    "{}",
                    serde_json::json!({"reset": false, "reason": "no config found"})
                );
            }
        }
        return Ok(());
    }

    if !force {
        use std::io::{self, BufRead, Write};
        print!("This will delete all keys and config. Are you sure? [y/N] ");
        io::stdout().flush()?;
        let mut input = String::new();
        io::stdin().lock().read_line(&mut input)?;
        if !matches!(input.trim().to_lowercase().as_str(), "y" | "yes") {
            println!("Aborted.");
            return Ok(());
        }
    }

    let path = config::config_path()?;
    config::delete_config()?;

    match output {
        OutputFormat::Table => {
            println!("Config deleted: {}", path.display());
            println!("All keys and settings have been removed.");
        }
        OutputFormat::Json => {
            println!(
                "{}",
                serde_json::json!({
                    "reset": true,
                    "deleted": path.display().to_string(),
                })
            );
        }
    }
    Ok(())
}

async fn cmd_sync(
    output: OutputFormat,
    private_key_flag: Option<&str>,
    address_arg: Option<polymarket_client_sdk::types::Address>,
) -> Result<()> {
    let eoa = match address_arg {
        Some(addr) => addr,
        None => {
            let (key, _) = config::resolve_key(private_key_flag)?;
            let key = key.ok_or_else(|| anyhow::anyhow!("{}", config::NO_WALLET_MSG))?;
            let signer = LocalSigner::from_str(&key).context("Invalid private key")?;
            signer.address()
        }
    };

    let gamma = polymarket_client_sdk::gamma::Client::default();
    let req = PublicProfileRequest::builder().address(eoa).build();
    let profile = gamma.public_profile(&req).await
        .context(format!(
            "Failed to fetch Polymarket profile for {eoa}. \
             If your Polymarket account uses a different EOA, pass it as an argument: \
             polymarket wallet sync <EOA_ADDRESS>"
        ))?;

    let proxy = profile
        .proxy_wallet
        .ok_or_else(|| anyhow::anyhow!(
            "No proxy wallet found in Polymarket profile for {eoa}. \
             This account may not have a proxy wallet deployed yet."
        ))?;

    let existing = config::load_config()?
        .ok_or_else(|| anyhow::anyhow!("{}", config::NO_WALLET_MSG))?;
    config::save_wallet_with_funder(
        &existing.private_key,
        existing.chain_id,
        &existing.signature_type,
        Some(&proxy.to_string()),
    )?;

    match output {
        OutputFormat::Json => {
            println!(
                "{}",
                serde_json::json!({
                    "funder": proxy.to_string(),
                    "eoa": eoa.to_string(),
                })
            );
        }
        OutputFormat::Table => {
            println!("Funder set to: {proxy}");
            println!("EOA:           {eoa}");
        }
    }
    Ok(())
}
