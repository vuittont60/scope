use anchor_client::solana_sdk::signature::read_keypair_file;
use anchor_client::{solana_sdk::pubkey::Pubkey, Client, Cluster};

use scope_client::{ScopeClient, TokenConfList};
use solana_sdk::clock;
use solana_sdk::commitment_config::CommitmentConfig;
use std::path::Path;
use std::path::PathBuf;
use std::rc::Rc;
use std::thread::sleep;
use std::time::{Duration, Instant};

use clap::{Parser, Subcommand};

use tracing::{error, info, trace, warn};

use anyhow::Result;

#[derive(Parser, Debug)]
#[clap(author, version, about, long_about = None)]
struct Args {
    /// Connect to solana validator
    #[clap(long, env, parse(try_from_str), default_value = "localnet")]
    cluster: Cluster,

    /// Account keypair to pay for the transactions
    #[clap(long, env, parse(from_os_str))]
    keypair: PathBuf,

    /// Program Id
    #[clap(long, env, parse(try_from_str))]
    program_id: Pubkey,

    /// "Price feed" unique name to work with
    #[clap(long, env)]
    price_feed: String,

    /// Subcommand to execute
    #[clap(subcommand)]
    action: Actions,
}

#[derive(Debug, Subcommand)]
enum Actions {
    /// Download the remote oracle mapping in the provided mapping file
    #[clap(arg_required_else_help = true)]
    Download {
        /// Where to store the mapping
        #[clap(long, env, parse(from_os_str))]
        mapping: PathBuf,
    },
    /// Upload the provided oracle mapping to the chain.
    /// This requires initial program deploy account
    #[clap(arg_required_else_help = true)]
    Upload {
        /// Where is stored the mapping to upload
        #[clap(long, env, parse(from_os_str))]
        mapping: PathBuf,
    },
    /// Initialize the program accounts
    /// This requires initial program deploy account and enough funds
    #[clap()]
    Init {
        /// Where is stored the mapping to use
        #[clap(long, env, parse(from_os_str))]
        mapping: Option<PathBuf>,
    },

    /// Automatically refresh the prices
    #[clap()]
    Crank {
        #[clap(long, env, default_value = "5")]
        refresh_interval_slot: clock::Slot,
        /// Where to store the mapping
        #[clap(long, env, parse(from_os_str))]
        mapping: Option<PathBuf>,
    },
}

fn main() -> Result<()> {
    let args: Args = Args::parse();

    // TODO output JSON?
    tracing_subscriber::fmt::init();

    // Read keypair to sign transactions
    let payer = read_keypair_file(args.keypair).expect("Keypair file not found or invalid");

    let commitment = if let Actions::Crank { .. } = args.action {
        // For crank we don't want to wait for proper confirmation of the refresh transaction
        CommitmentConfig::confirmed()
    } else {
        CommitmentConfig::finalized()
    };

    let client = Client::new_with_options(args.cluster, Rc::new(payer), commitment);

    if let Actions::Init { mapping } = args.action {
        init(&client, &args.program_id, &args.price_feed, &mapping)
    } else {
        let mut scope = ScopeClient::new(client, args.program_id, &args.price_feed)?;

        match args.action {
            Actions::Download { mapping } => download(&mut scope, &mapping),
            Actions::Upload { mapping } => upload(&mut scope, &mapping),
            Actions::Init { .. } => unreachable!(),
            Actions::Crank {
                refresh_interval_slot,
                mapping,
            } => crank(&mut scope, (&mapping).as_ref(), refresh_interval_slot),
        }
    }
}

fn init(
    client: &Client,
    program_id: &Pubkey,
    price_feed: &str,
    mapping_op: &Option<impl AsRef<Path>>,
) -> Result<()> {
    let mut scope = ScopeClient::new_init_program(client, program_id, price_feed)?;

    if let Some(mapping) = mapping_op {
        let token_list = TokenConfList::read_from_file(&mapping)?;
        scope.set_local_mapping(&token_list)?;
        scope.upload_oracle_mapping()?;
    }

    Ok(())
}

fn upload(scope: &mut ScopeClient, mapping: &impl AsRef<Path>) -> Result<()> {
    let token_list = TokenConfList::read_from_file(&mapping)?;
    scope.set_local_mapping(&token_list)?;
    scope.upload_oracle_mapping()
}

fn download(scope: &mut ScopeClient, mapping: &impl AsRef<Path>) -> Result<()> {
    scope.download_oracle_mapping()?;
    let token_list = scope.get_local_mapping()?;
    token_list.save_to_file(&mapping)
}

fn crank(
    scope: &mut ScopeClient,
    mapping_op: Option<impl AsRef<Path>>,
    refresh_interval_slot: clock::Slot,
) -> Result<()> {
    info!("Refresh interval set to {:?} slots", refresh_interval_slot);

    if let Some(mapping) = mapping_op {
        let token_list = TokenConfList::read_from_file(&mapping)?;
        scope.set_local_mapping(&token_list)?;
        // TODO add check if local is correctly equal to remote mapping
    } else {
        scope.download_oracle_mapping()?;
    }
    loop {
        let start = Instant::now();

        if let Err(e) = scope.refresh_prices_older_than(refresh_interval_slot) {
            error!("Error while refreshing prices {:?}", e);
        }

        let elapsed = start.elapsed();
        trace!("last refresh duration was {:?}", elapsed);

        let oldest_age = scope.get_oldest_price_age()?;
        trace!(oldest_age);

        if refresh_interval_slot > oldest_age {
            let sleep_ms = (refresh_interval_slot - oldest_age) * clock::DEFAULT_MS_PER_SLOT;
            sleep(Duration::from_millis(sleep_ms));
        } else {
            warn!("No sleep, some prices are already too old");
        }
    }

    #[allow(unreachable_code)]
    {
        // no exit condition in crank operating mode
        unreachable!()
    }
}
