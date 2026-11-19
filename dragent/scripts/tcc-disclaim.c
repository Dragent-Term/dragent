// Exec a binary as its OWN TCC "responsible process".
//
// When a dev binary is launched from a terminal (Terminal.app, Cursor, VS
// Code…), macOS attributes privacy permission checks to that TERMINAL — and
// the usage-description lookup happens against the terminal's Info.plist.
// Cursor has no NSSpeechRecognitionUsageDescription, so our speech request
// was killed by TCC no matter what our own (embedded, signature-bound)
// Info.plist said.
//
// responsibility_spawnattrs_setdisclaim severs that inheritance: the spawned
// process becomes self-responsible, so TCC consults ITS plist and shows the
// permission prompt under its own name. Private-but-stable API, the same
// mechanism Chromium and Alacritty use for exactly this problem.
//
// POSIX_SPAWN_SETEXEC makes this an exec-replacement (same pid, same stdio),
// so `cargo run` behaves exactly as before.

#include <spawn.h>
#include <stdio.h>

extern int responsibility_spawnattrs_setdisclaim(posix_spawnattr_t *attrs, int disclaim);
extern char **environ;

int main(int argc, char **argv) {
    if (argc < 2) {
        fprintf(stderr, "usage: tcc-disclaim <binary> [args...]\n");
        return 1;
    }
    posix_spawnattr_t attr;
    posix_spawnattr_init(&attr);
    responsibility_spawnattrs_setdisclaim(&attr, 1);
    posix_spawnattr_setflags(&attr, POSIX_SPAWN_SETEXEC);
    posix_spawn(NULL, argv[1], NULL, &attr, &argv[1], environ);
    // Only reached if the spawn-exec failed.
    perror("tcc-disclaim: posix_spawn");
    return 127;
}
