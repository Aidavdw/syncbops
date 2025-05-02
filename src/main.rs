mod ffmpeg_interface;
mod hashing;
mod music_library;
mod song;
mod sync_song;
#[cfg(test)]
mod test_data;
use clap::{arg, Parser};
use dialoguer::Confirm;
use hashing::{
    read_records_of_previous_sync, register_record_to_previous_sync_db,
    write_records_of_current_sync, SyncRecord,
};
use indicatif::{ParallelProgressIterator, ProgressBar, ProgressStyle};
use music_library::{
    copy_dedicated_cover_art_for_song, find_songs_in_library, ArtStrategy, ArtworkType,
    MusicFileType, MusicLibraryError, UpdateType,
};
use rayon::iter::{IntoParallelRefIterator, ParallelIterator};
use song::Song;
use std::{
    path::{Path, PathBuf},
    process::exit,
};
use sync_song::sync_song;

/// What all the individual attempts at syncing are collected into.
type SyncResults<'a> = Vec<(&'a Song, Result<SyncRecord, MusicLibraryError>)>;

const PREVIOUS_SYNC_DB_FILENAME: &str = ".syncbops";

#[derive(clap::Parser)]
#[command(version, about, long_about = None)] // Read from cargo.toml
struct Cli {
    #[command(subcommand)]
    target_filetype: MusicFileType,

    /// The directory to be scanned for music files to synchronise
    source_library: PathBuf,

    /// The directory that a transcoded copy of the library provided will be put into.
    target_library: PathBuf,

    /// Force overwriting existing music files. Does not affect external album art files.
    #[arg(short, long, default_value_t = false)]
    force: bool,

    /// How to handle album art
    #[arg(short, long, value_name = "STRATEGY", default_value = "prefer-file")]
    art_strategy: ArtStrategy,

    /// Don't actually make any changes to the filesystem, just report on what it would look like after the operation. Makes most sense to run together with verbose option.
    #[arg(short, long, default_value_t = false)]
    dry_run: bool,

    /// Display more info.
    #[arg(short, long, default_value_t = false)]
    verbose: bool,

    /// Automatically say 'yes' to any prompts that show up.
    #[arg(short, long, default_value_t = false)]
    yes: bool,

    /// Maximum amount of threads to use. If no value given, will use all threads.
    #[arg(short, long)]
    thread_count: Option<usize>,

    /// Disable writing of records of the current synchronisation run to the target library.
    /// future synchronising runs can be performed much faster if these are present, as file
    /// changes can be checked based on hashes.
    /// Disabling them makes updating much slower, but does not contaminate the target dir.
    #[arg(long, default_value_t = false)]
    dont_save_records: bool,
    // TODO: Maximum resolution for embedded art. Works like a threshold: Files larger than this resolution will be scaled, files lower in resolution will not be touched. 0 will not do any scaling, and embed everything at their actual resolution.

    // #[arg(short, long, value_name = "RESOLUTION", default_value_t = 0)]
    // embed_art_resolution: u64,
}

fn main() -> miette::Result<()> {
    let cli = Cli::parse();
    let source_library = cli.source_library;
    let target_library = cli.target_library;

    if cli.dry_run {
        println!("Performing a dry run, so no actual changes will be made to the filesystem.")
    }

    if let Some(x) = cli.thread_count {
        rayon::ThreadPoolBuilder::new()
            .num_threads(x)
            .build_global()
            .unwrap_or_else(|_| panic!("Cannot set amount of threads to {}. Exiting.", x));
    }

    println!("Discovering files in {}", source_library.display());
    let songs = find_songs_in_library(&source_library)?;
    println!("Discovered {} songs.", songs.len());

    // It would really suck to accidentally overwrite your main library with your transcoded
    // stuff by mixing up the source dir and target dir. So, here are some guardrails to make
    // it much harder for that to happen:
    // Ask for confirmation if:
    // 1. there exists a database file in this directory (this is indicative of this being a
    //    target lib)
    // 2. there are many high-bitrate songs in this library.
    if !cli.yes {
        let there_are_records_in_source_library =
            source_library.join(PREVIOUS_SYNC_DB_FILENAME).exists();
        let there_are_many_high_bitrate_songs = songs
            .iter()
            .filter(|song| song.metadata.bitrate_kbps > 260)
            .count()
            > 100;
        if there_are_records_in_source_library || there_are_many_high_bitrate_songs {
            let confirmation = Confirm::new()
                .with_prompt("The provided source library contains records from a previous sync. You might have mixed up the source directory and the target directory! Do you want to continue anywan?")
                .default(false)
                .interact()
                .unwrap();

            if confirmation {
                println!("Continuing anyway!");
            } else {
                println!("Aborting. Saved your music library!");
                exit(0);
            }
        }
    }

    // Report if there are songs without album art.
    println!("Checking for songs without album art...");
    let songs_without_album_art = songs_without_album_art(&songs);
    if !songs_without_album_art.is_empty() {
        println!("Warning! There are songs without any album art (either embedded or found in Cover.jpg, folder.png, etc:");
        for x in songs_without_album_art {
            println!("\t- {}", x)
        }
    }

    // If the target dir does not exist, warn the user that it does not exist. Don't just
    // willy-nilly create it, because they could've made a typo.
    if !target_library.is_dir() {
        return Err(MusicLibraryError::TargetLibraryDoesNotExist {
            target_library: target_library.clone(),
        }
        .into());
    }

    let art_strategy = cli.art_strategy;

    // Load the results from the last hash.
    let previous_sync_db = read_records_of_previous_sync(&target_library);
    let records_found = previous_sync_db.is_some();

    // Do the synchronising on a per-file basis, so that it can be parallelised. Each one starting
    // with its own ffmpeg thread.
    println!("Synchronising music files...");
    if cli.force {
        println!("Forced re-writing every music file.")
    }
    let pb = ProgressBar::new(songs.len() as u64);
    pb.set_style(
        ProgressStyle::default_bar()
            .template("[{elapsed}] [{bar:60.cyan/blue}] {pos}/{len} [ETA: {eta}] {msg}")
            .unwrap()
            .progress_chars("#>-"),
    );
    let sync_results: SyncResults = songs
        .par_iter()
        .progress_with(pb.clone())
        .map(|song| {
            pb.set_message(format!("{}", song.library_relative_path.display()));
            (
                song,
                sync_song(
                    song,
                    &target_library,
                    cli.target_filetype.clone(),
                    art_strategy,
                    previous_sync_db.as_ref(),
                    cli.force,
                    cli.dry_run,
                    Some(&pb),
                ),
            )
        })
        .collect::<SyncResults>();

    // Might be sorted differently because of parallel execution, so put in alphabetic order again.
    let sync_results = {
        let mut unsorted = sync_results;
        unsorted.sort_by(|(i_a, _), (i_b, _)| i_a.absolute_path.cmp(&i_b.absolute_path));
        unsorted
    };

    // Go over all the dedicated album art.
    // If there is a dedicated art file for the music file, add it. If it already exists, it is probably already added by another file
    let new_cover_arts = if !cli.dry_run {
        println!("Checking and copying external cover art...");
        Some(
            songs
                .iter()
                .map(|song| {
                    copy_dedicated_cover_art_for_song(
                        song,
                        &source_library,
                        &target_library,
                        cli.dry_run,
                    )
                })
                .collect::<Result<Vec<_>, _>>()?
                .iter()
                .filter_map(|o| o.to_owned())
                .collect::<Vec<_>>(),
        )
    } else {
        None
    };

    // Update the PreviousSyncDB with the newly added items.
    if !cli.dont_save_records && !cli.dry_run {
        println!("Writing new records so the next sync can be done faster");
        // Carry over any previous records (files that are not touched retain their original data).
        let mut new_records = previous_sync_db.unwrap_or_default();

        for (_song, update_result) in &sync_results {
            let Ok(record) = update_result else {
                // Can't update syncdb if it errored.
                continue;
            };
            debug_assert!(record.update_type.is_some());
            // NOTE: If miette could work with references, I could instead do printing a summary first,
            // and then owned move the records into the db.
            // Not the case, so a .clone() is necessary here.
            register_record_to_previous_sync_db(&mut new_records, record.to_owned())
        }
        // TODO: Also handle deleting songs. Right now it only adds one-way lol. For every filename in
        // the target directory, check if the same filename -prefix exists in the source dir, otherwise
        // delete it. can re-use find_albums_in_directory()
        write_records_of_current_sync(&new_records, &target_library);
    }

    print!("{}", summarize(sync_results, new_cover_arts, cli.verbose));
    if !cli.dry_run {
        print_library_size_reduction(&source_library, &target_library);
    }

    // If not writing any records, but there are records present, the synchronisation state in
    // those is no longer up to date. Warn the user of this.
    if cli.dont_save_records && records_found {
        println!("Writing records is disabled, but there are already records present in the target directory (from a previous run?). This means that the next synchronisation will use this data, and not update everything. It is therefore recommended to delete the existing records file from the target library.")
    }
    Ok(())
    // TODO: Separately search for "albumname.jpg" everywhere. Match this to the albums by
    // reading their tags, and link it if the album does not yet have art set.
}

pub fn songs_without_album_art(songs: &[Song]) -> Vec<&Song> {
    let yee = songs
        .iter()
        .filter(|song| song.has_artwork() == ArtworkType::None)
        .collect::<Vec<_>>();
    yee
}

fn summarize(
    sync_results: SyncResults,
    new_cover_arts: Option<Vec<PathBuf>>,
    verbose: bool,
) -> String {
    // Thif function should use an owned SyncResults, because otherwise you can't get nice
    // miette::report
    let mut summary = String::with_capacity(4000);
    let mut n_unchanged = 0;
    let mut n_new = 0;
    let mut n_overwritten = 0;
    let mut n_err = 0;
    let mut n_missing_target = 0;
    let mut n_copied = 0;
    let mut error_messages = if verbose {
        String::with_capacity(50000)
    } else {
        String::new()
    };
    let mut song_updates = String::new();
    for (song, r) in sync_results {
        match r {
            Ok(sync_record) => {
                let update_type = sync_record
                    .update_type
                    .expect("Empty update type. Implementation error");
                use UpdateType as U;
                match update_type {
                    U::NoChange => n_unchanged += 1,
                    U::NewTranscode => n_new += 1,
                    U::Overwrite => n_overwritten += 1,
                    U::ForceOverwrite => n_overwritten += 1,
                    U::TranscodeMissingTarget => n_missing_target += 1,
                    U::Copied => n_copied += 1,
                };
                song_updates.push_str(&format!(
                    "{} →  [{:?}]\n",
                    song.absolute_path.display(),
                    update_type
                ))
            }
            Err(e) => {
                n_err += 1;
                let err_msg = &format!(
                    "{} →  [Error]\n{:?}\n",
                    song.absolute_path.display(),
                    miette::Report::new(e)
                );
                error_messages.push_str(err_msg)
            }
        }
    }
    summary.push_str("====== Summary of synchronisation ======\n");
    summary.push_str(&format!("Unchanged: {}\n", n_unchanged));
    summary.push_str(&format!("New songs: {}\n", n_new));
    summary.push_str(&format!("Changed songs (overwritten): {}\n", n_overwritten));
    summary.push_str(&format!("Re-added missing: {}\n", n_missing_target));
    summary.push_str(&format!("Copied (not transcoded): {}\n", n_copied));
    if let Some(art_files) = new_cover_arts {
        summary.push_str(&format!("New album art: {}\n", art_files.len()));
    }
    if n_err == 0 {
        summary.push_str("No Errors :D\n");
    } else {
        summary.push_str(&format!("Files with errors: {}\n", n_err));
        summary.push_str("The following errors occurred:\n");
        summary.push_str(&error_messages);
    }
    if verbose {
        summary.push_str("Change log\n");
        summary.push_str(&song_updates)
    }

    summary
}

fn print_library_size_reduction(source_library: &Path, target_library: &Path) {
    use fs_extra::dir::get_size;
    let source_lib_size = get_size(source_library).unwrap();
    let target_lib_size = get_size(target_library).unwrap();
    let percentage_reduction = (target_lib_size) as f64 / source_lib_size as f64 * 100.;
    println!(
        "Reduced library from {} MB to {} MB ({:.2}%)",
        source_lib_size / 1_000_000,
        target_lib_size / 1_000_000,
        percentage_reduction
    )
}

/// Called to log whenever an operation has failed on a music file, but the program is allowed to
/// continue running.
/// To death with silent errors!
/// Wraps log and println because of https://github.com/console-rs/indicatif/issues/474
pub fn log_failure(msg: String, pb: Option<&ProgressBar>) {
    // TODO: Also add logging to file
    if let Some(progress_bar) = pb {
        progress_bar.println(msg)
    } else {
        eprintln!("{}", msg)
    }
}
