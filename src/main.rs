mod ffmpeg_interface;
mod music_library;
mod song;
use clap::{arg, Parser};
use indicatif::ParallelProgressIterator;
use music_library::{
    copy_dedicated_cover_art_for_song, find_albums_in_directory, songs_without_album_art,
    sync_song, ArtStrategy, MusicFileType, MusicLibraryError, UpdateType,
};
use rayon::iter::{IntoParallelRefIterator, ParallelIterator};
use song::Song;
use std::path::PathBuf;

/// What all the individual attempts at syncing are collected into.
type SyncResults<'a> = Vec<(&'a Song, Result<UpdateType, MusicLibraryError>)>;

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

    /// TODO: Maximum resolution for embedded art. Works like a threshold: Files larger than this resolution will be scaled, files lower in resolution will not be touched. 0 will not do any scaling, and embed everything at their actual resolution.
    #[arg(short, long, value_name = "RESOLUTION", default_value_t = 0)]
    embed_art_resolution: u64,

    /// Don't actually make any changes to the filesystem, just report on what it would look like after the operation. Makes most sense to run together with verbose option.
    #[arg(short, long, default_value_t = false)]
    dry_run: bool,

    /// Display more info.
    #[arg(short, long, default_value_t = false)]
    verbose: bool,
}

fn main() -> miette::Result<()> {
    let cli = Cli::parse();
    let source_library = cli.source_library;
    let target_library = cli.target_library;

    // TODO: Validate if e.g. FLAC level is between 0 and 12, otherwise return error.
    match cli.target_filetype {
        MusicFileType::Mp3 { .. } => (),
        _ => return Err(MusicLibraryError::OutputCodecNotYetImplemented.into()),
    }

    if cli.dry_run {
        println!("Performing a dry run, so no actual changes will be made to the filesystem.")
    }

    println!("Discovering files in {}", source_library.display());
    let albums = find_albums_in_directory(&source_library, cli.verbose)?;
    println!(
        "Discovered {} songs in {} folders.",
        albums
            .iter()
            .map(|album| album.music_files.len())
            .sum::<usize>(),
        albums.len()
    );
    // Report if there are songs without album art.
    println!("Checking for songs without album art...");
    let songs_without_album_art = songs_without_album_art(&albums)?;
    if !songs_without_album_art.is_empty() {
        println!("Warning! There are songs without any album art (either embedded or found in Cover.jpg, folder.png, etc:");
        for x in songs_without_album_art {
            println!("\t- {}", x.display())
        }
    }

    // Convert Albums to Songs
    let mut songs = Vec::new();
    for album in albums {
        songs.extend(album.music_files.iter().map(|music_file| Song {
            path: music_file.to_path_buf(),
            external_album_art: album.album_art.clone(),
        }));
    }
    let songs = songs; // unmut

    // If the target dir coes not exist, warn the user that it does not exist. Don't just
    // willy-nilly create it, because they could've made a typo.
    if !target_library.is_dir() {
        return Err(MusicLibraryError::TargetLibraryDoesNotExist {
            target_library: target_library.clone(),
        }
        .into());
    }

    let art_strategy = cli.art_strategy;

    // Do the synchronising on a per-file basis, so that it can be parallelised. Each one starting
    // with its own ffmpeg thread.
    println!("Synchronising music files...");
    if cli.force {
        println!("Forced re-writing every music file.")
    }
    let sync_results: SyncResults = songs
        .par_iter()
        .progress()
        .map(|song| {
            (
                song,
                sync_song(
                    song,
                    &source_library,
                    &target_library,
                    cli.target_filetype.clone(),
                    art_strategy,
                    cli.force,
                    cli.dry_run,
                ),
            )
        })
        .collect::<SyncResults>();

    // Go over all the dedicated album art.
    // If there is a dedicated art file for the music file, add it. If it already exists, it is probably already added by another file
    println!("Checking and copying external cover art...");
    let new_cover_arts = songs
        .iter()
        .map(|song| {
            copy_dedicated_cover_art_for_song(song, &source_library, &target_library, cli.dry_run)
        })
        .collect::<Result<Vec<_>, _>>()?
        .iter()
        .filter_map(|o| o.to_owned())
        .collect::<Vec<_>>();

    print!("{}", summarize(sync_results, new_cover_arts, cli.verbose));

    Ok(())
    // TODO: Separately search for "albumname.jpg" everywhere. Match this to the albums by
    // reading their tags, and link it if the album does not yet have art set.

    // TODO: Also handle deleting songs. Right now it only adds one-way lol. For every filename in
    // the target directory, check if the same filename -prefix exists in the source dir, otherwise
    // delete it. can re-use find_albums_in_directory()
}

fn summarize(sync_results: SyncResults, new_cover_arts: Vec<PathBuf>, verbose: bool) -> String {
    // Might be sorted differently because of parallel execution, so put in order again.
    let mut unsorted = sync_results;
    unsorted.sort_by(|(i_a, _), (i_b, _)| i_a.path.cmp(&i_b.path));
    let sync_results = unsorted;

    let mut summary = String::with_capacity(4000);
    let mut n_unchanged = 0;
    let mut n_new = 0;
    let mut n_overwritten = 0;
    let mut n_err = 0;
    let mut error_messages = if verbose {
        String::with_capacity(50000)
    } else {
        String::new()
    };
    let mut song_updates = String::new();
    for (song, r) in sync_results {
        match r {
            Ok(update_type) => {
                match update_type {
                    UpdateType::Unchanged => n_unchanged += 1,
                    UpdateType::New => n_new += 1,
                    UpdateType::Overwritten => n_overwritten += 1,
                };
                song_updates.push_str(&format!("{} →  [{:?}]\n", song.path.display(), update_type))
            }
            Err(e) => {
                n_err += 1;
                let err_msg = &format!(
                    "{} →  [Error]\n{:?}\n",
                    song.path.display(),
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
    summary.push_str(&format!("New album art: {}\n", new_cover_arts.len()));
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

    // TODO: Give a little message of "input folder was n gig, output is n gig. space saved: n %"
}
