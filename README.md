A tool to easily maintain a smaller-size copy of your music library, and keep it in sync with your main library.
The obvious use case for this is to be able to have a large music library on your main PC, but keep a lower-quality (but smaller filesize) version of the same library on a mobile device. 

# Features
- Transcodes files to a smaller format
- Handles many music filetypes:
    - MP3
    - FLAC
    - ogg 
    - m4a
- Output encoding can be selected from the above
- Plays well with mixed-encoding music libraries as input (MP3 + FLAC, etc)
- Directly copies files that are already of lower quality than the required quality instead of transcoding them.
- Multi-threaded querying and encoding
- Only re-encodes files that have been changed. Leaves unchanged files untouched.
- Select how to handle album art
- Dry run for if you want to try it out first.

# How to use
It should be pretty simple!
`syncbops /path/to/source/library /path/to/target/library <encoding>`

`syncbops` takes two main arguments:
First is a source library, which is the folder where you keep all your music (e.g. `~/Music/` on linux, or `C:/Users/<username>/Music` on windows). This folder will not be touched, it will just be read.
Then the target library, which is where you want to synchronise to. This can be another folder on your computer, a folder on your phone, or anything else really!

Finally, you should pass the encoding you want to have.
This can be e.g. `mp3-vbr`, `ogg`, etc.

There are several other things you can tweak about the behaviour of the program by passing flags and optional arguments. Check them at `syncbops --help`.

## Output formats
You can further customise the target encoding, e.g. to set a specific target bitrate. Check these out with for example `syncbops <source_lib> <target_lib> mp3-vbr --help`.
Think of values like bitrates for fixed bitrate formats, quality factors for variable bitrate formats, and compression for FLAC.
If you don't supply these explicitly, (sane) default values will be used.

## Art Strategy
The album art can be supplied in two different ways:
- Embedded into the file
- External

You can choose how you want to have album art in the synchronised library.
- none: Remove all embedded album art, and don't copy album art files
- embed-all:   Embeds album art in all files. Carries over album art that was already in source files, and embeds external album art. Might take up more space!
- prefer-file: If there is both embedded and external, prefer external. E.g. If there is a cover.jpg (or similar), use that. If there is no dedicated file, use embedded art
- file-only:   Do not embed any cover art: Discard all existing embedded art, only keep cover.jpg if it exists

## Records
Checking if a file has changed requires a lot of reading from disk and comparing two files, which is not very fast.
Saving the state of the first file at the time of synchronising to a little file can help speed this up.
By default, such a file is written to the target directory and used in consecutive synchronisation runs (and updated).
If the records are not present, either because you explicitly told the program not to write records, or because you deleted them manually, the fallback method is used instead.

