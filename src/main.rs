// For internal use within the CLI application only
pub(crate) mod cli;
pub(crate) mod constants;

// Export as a library
pub mod download;

use anyhow::{bail, Result};
use download::{
    crawl,
    fetch::{self, DownloadRecursiveStatus},
    types::{CrawlingState, Node, StateStore},
};
use std::fs;

#[tokio::main]
async fn main() -> Result<()> {
    // The working directory
    let pwd = std::env::current_dir()
        .unwrap()
        .to_str()
        .unwrap()
        .to_owned();

    // Parse the command line parameters into arg-matches
    let matches = cli::configure_parser(&pwd).get_matches();

    // Print the name and version of the application along its license notice
    println!("{} {}", constants::NAME, constants::VERSION);
    println!("{}\n", constants::LICENSE);

    // Try to extract the desired configuration from the arg-matches
    let cli_options = cli::get_options(matches)?;

    // Make a new client for issuing HTTP(S) requests
    let client = reqwest::Client::new();

    // Crawl the root directory
    // TODO extract to `setup` or `crawl` function
    let (mut state_store, state_path, mut done_list) = if let Some(state_path) =
        cli_options.state_store_path.clone()
    {
        // A state store is desired

        // Try to load the state store from the file system
        let mut state_store: StateStore =
            serde_json::from_str(&fs::read_to_string(&state_path).unwrap_or("make-new".to_owned()))
                .unwrap_or(StateStore::new());

        // Clone the done_list
        let done_list: Vec<String> = state_store.downloaded_urls.clone();

        // Return the pre-made crawl list or start crawling
        match state_store.crawling_state {
            CrawlingState::Complete(_) => (state_store, Some(state_path), done_list),
            CrawlingState::Partial(_) | CrawlingState::None => {
                // Perform the crawl
                // TODO utilize partial crawls in the future

                let mut root = crawl::get_root_dir(&cli_options.url, &client).await?;

                // Expand the tree
                if let Node::CrawledDir(_, ref mut children) = root {
                    crawl::expand_node(children, &client).await?;
                } else {
                    panic!("Cannot expand root node")
                }

                // Update the modified time
                state_store.update_modified_time();

                // Save the completed crawl
                state_store.crawling_state = CrawlingState::Complete(root.clone());

                // Serialize & persist the new state store
                fs::write(&state_path, serde_json::to_string_pretty(&state_store)?)
                    .expect("Cannot write to state store");

                // Return the crawl results
                (state_store, Some(state_path), done_list)
            }
        }
    } else {
        // No state store is desired

        // Check if --no-download was specified
        if cli_options.no_download {
            bail!("Cannot use --no-download without --state-store")
        }

        // Make a phantom state store (not persisted)
        let mut state_store = StateStore::new();

        let mut root = crawl::get_root_dir(&cli_options.url, &client).await?;

        // Expand the tree
        if let Node::CrawledDir(_, ref mut children) = root {
            crawl::expand_node(children, &client).await?;
        } else {
            panic!("Cannot expand root node")
        }

        // Save the completed crawl
        state_store.crawling_state = CrawlingState::Complete(root.clone());

        (state_store, None, vec![])
    };

    // Only download files if --no-download was not specified
    // TODO extract to `download_files` function
    if !cli_options.no_download {
        // TODO implement the counters
        let mut counters = download::fetch::LimitCounts::new();
        let mut counters_1 = counters.clone();

        let res = {
            fetch::download_recursive(
                state_store.get_root_ref()?,
                &cli_options,
                &client,
                &mut counters_1,
                &mut done_list,
            )
            .await?
        };

        if let DownloadRecursiveStatus::Do(ref to_do) = res {
            for task in to_do {
                let (node, options, client) = task;
                // TODO implement more than one level of recursion
                // res = fetch::download_recursive(node, options, client, &mut counters).await?;
                match fetch::download_recursive(
                    node,
                    options,
                    client,
                    &mut counters,
                    &mut done_list,
                )
                .await
                {
                    Ok(_) => {}
                    Err(error) => {
                        if let Some(state_path) = state_path {
                            // Update the modified time
                            state_store.update_modified_time();

                            // Update the done_list
                            state_store.downloaded_urls = done_list;

                            // Serialize & persist the new state store
                            fs::write(state_path, serde_json::to_string_pretty(&state_store)?)
                                .expect("Cannot write to state store");
                        }

                        // Return the error and halt execution
                        bail!(error)
                    }
                }
            }
        }
    }

    // Persist the new state to disk if necessary
    if let Some(state_path) = state_path {
        write_state(&mut state_store, &state_path, done_list)?;
        println!("Download done.");
    } else {
        println!("All done.");
    }

    Ok(())
}

/// Persists the state to disk
fn write_state(
    state_store: &mut StateStore,
    state_path: &str,
    done_list: Vec<String>,
) -> Result<()> {
    // Update the modified time
    state_store.update_modified_time();

    // Update the done_list
    state_store.downloaded_urls = done_list;

    // Serialize & persist the new state store
    fs::write(state_path, serde_json::to_string_pretty(&state_store)?)
        .expect("Cannot write to state store");

    println!("Wrote state store to {}", state_path);

    Ok(())
}
