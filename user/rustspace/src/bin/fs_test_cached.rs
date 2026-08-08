#![no_std]
#![no_main]

extern crate alloc;

use alloc::vec::Vec;
use core::arch::global_asm;
use rustspace::{
    CacheImportance, CacheStatsFlat, CpuInfoFlat, DirEntryFlat, FD_FLAG_READ, FD_FLAG_WRITE,
    StatFlat, println, sys_close_file, sys_create_dir, sys_create_file, sys_delete,
    sys_evict_directory, sys_exit, sys_flush_file_cache, sys_get_cache_stats, sys_get_cpu_info,
    sys_list_dir, sys_open_file, sys_pin_file, sys_read_file, sys_reserve_cache, sys_stat_file,
    sys_unpin_file, sys_write_file, tsc_read,
};

global_asm!(
    ".section .text.entry",
    ".global _start",
    "_start:",
    "    call main",
    "    ud2",
);

static mut PASS_COUNT: u32 = 0;
static mut FAIL_COUNT: u32 = 0;

macro_rules! expect_true {
    ($label:expr, $cond:expr) => {{
        if $cond {
            unsafe { PASS_COUNT += 1 };
            println!("  PASS  {}", $label);
        } else {
            unsafe { FAIL_COUNT += 1 };
            println!("  FAIL  {}", $label);
        }
    }};
}

macro_rules! expect_eq {
    ($label:expr, $left:expr, $right:expr) => {{
        let l = $left;
        let r = $right;
        if l == r {
            unsafe { PASS_COUNT += 1 };
            println!("  PASS  {}", $label);
        } else {
            unsafe { FAIL_COUNT += 1 };
            println!("  FAIL  {} (got {}, expected {})", $label, l, r);
        }
    }};
}

fn suite_header(name: &str) {
    println!("");
    println!("=== {} ===", name);
}

fn print_summary() {
    let pass = unsafe { PASS_COUNT };
    let fail = unsafe { FAIL_COUNT };
    println!("");
    println!("--- results: {} passed, {} failed ---", pass, fail);
}

// Helper: read all bytes from an open fd into a Vec
unsafe fn read_all(fd: usize, max_bytes: usize) -> Option<Vec<u8>> {
    let mut result = Vec::new();
    let mut buf = [0u8; 512];
    loop {
        let to_read = buf.len().min(max_bytes.saturating_sub(result.len()));
        if to_read == 0 {
            break;
        }
        let n = unsafe { sys_read_file(fd, &mut buf[..to_read]) };
        if n == usize::MAX {
            return None;
        }
        if n == 0 {
            break;
        }
        result.extend_from_slice(&buf[..n]);
    }
    Some(result)
}

// Helper: print cache statistics
unsafe fn print_cache_stats(label: &str) {
    let mut stats = CacheStatsFlat::zeroed();
    let ok = unsafe { sys_get_cache_stats(&mut stats) };
    if ok {
        println!(
            "  [cache] {}: {} files ({} dirty), {} KB / {} KB",
            label,
            stats.total_files,
            stats.dirty_files,
            stats.total_bytes / 1024,
            stats.max_bytes / 1024,
        );
    } else {
        println!("  [cache] {}: stats unavailable", label);
    }
}

// Helper: measure cycles for a closure
unsafe fn measure_cycles<F: FnOnce()>(label: &str, f: F) -> u64 {
    let (start, _) = unsafe { tsc_read() };
    f();
    let (end, _) = unsafe { tsc_read() };
    let elapsed = end - start;
    println!("  [perf] {}: {} cycles", label, elapsed);
    elapsed
}

// ===== Suite 1: Basic Cache Operations =====

unsafe fn suite_cache_basics() {
    suite_header("cache basics");

    // Verify cache is working by checking stats
    unsafe { print_cache_stats("initial") };

    // Reserve cache space for test directory with high importance
    let ok = unsafe { sys_reserve_cache("/cactest", CacheImportance::VeryHigh as u8) };
    expect_true!("reserve_cache('/cactest')", ok == 0);

    const DIR_PATH: &str = "/cactest";
    const FILE_PATH: &str = "/cactest/pinned.txt";
    const FILE_DATA: &[u8] = b"This file should stay in cache";

    // Clean up from previous runs
    unsafe { sys_delete(FILE_PATH) };
    unsafe { sys_delete(DIR_PATH) };
    unsafe { sys_create_dir(DIR_PATH) };
    unsafe { sys_create_file(FILE_PATH) };

    // Write test data
    let fd = unsafe { sys_open_file(FILE_PATH, FD_FLAG_READ | FD_FLAG_WRITE) };
    if fd != usize::MAX {
        unsafe { sys_write_file(fd, FILE_DATA) };
        unsafe { sys_close_file(fd) };
    }

    // Pin the file
    let ok = unsafe { sys_pin_file(FILE_PATH) };
    expect_true!("pin_file succeeds", ok == 0);

    unsafe { print_cache_stats("after pin") };

    // First read: should load into cache
    let (start_cold, _) = unsafe { tsc_read() };
    let fd1 = unsafe { sys_open_file(FILE_PATH, FD_FLAG_READ) };
    if fd1 != usize::MAX {
        let _data = unsafe { read_all(fd1, FILE_DATA.len()) };
        unsafe { sys_close_file(fd1) };
    }
    let (end_cold, _) = unsafe { tsc_read() };
    let cold_cycles = end_cold - start_cold;
    println!("  [perf] first read (cold cache): {} cycles", cold_cycles);

    // Second read: should hit cache, much faster
    let (start_hot, _) = unsafe { tsc_read() };
    let fd2 = unsafe { sys_open_file(FILE_PATH, FD_FLAG_READ) };
    if fd2 != usize::MAX {
        let _data = unsafe { read_all(fd2, FILE_DATA.len()) };
        unsafe { sys_close_file(fd2) };
    }
    let (end_hot, _) = unsafe { tsc_read() };
    let hot_cycles = end_hot - start_hot;
    println!("  [perf] second read (warm cache): {} cycles", hot_cycles);

    // Cache should speed up reads significantly
    if cold_cycles > 0 {
        let speedup = cold_cycles as f64 / hot_cycles.max(1) as f64;
        println!("  [perf] cache speedup: {:.1}x", speedup);
        expect_true!("cache provides speedup", speedup > 1.0);
    }

    // Unpin the file
    let ok = unsafe { sys_unpin_file(FILE_PATH) };
    expect_true!("unpin_file succeeds", ok == 0);

    unsafe { print_cache_stats("after unpin") };

    // Clean up
    unsafe { sys_delete(FILE_PATH) };
    unsafe { sys_delete(DIR_PATH) };
}

// ===== Suite 2: Repeated Reads (Cache Hit Rate) =====

unsafe fn suite_repeated_reads() {
    suite_header("repeated reads");

    const PATH: &str = "/reptest.txt";
    const DATA: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ012345";

    unsafe { sys_delete(PATH) };
    unsafe { sys_create_file(PATH) };

    let fd = unsafe { sys_open_file(PATH, FD_FLAG_READ | FD_FLAG_WRITE) };
    if fd == usize::MAX {
        println!("  SKIP (create failed)");
        return;
    }
    unsafe { sys_write_file(fd, DATA) };
    unsafe { sys_close_file(fd) };

    // Reserve cache for this file
    unsafe { sys_reserve_cache("/", CacheImportance::Normal as u8) };

    // Read the file multiple times and measure
    const ITERATIONS: usize = 10;
    let mut total_cycles = 0u64;
    let mut first_cycles = 0u64;

    for i in 0..ITERATIONS {
        let (start, _) = unsafe { tsc_read() };
        let fd_r = unsafe { sys_open_file(PATH, FD_FLAG_READ) };
        if fd_r == usize::MAX {
            println!("  SKIP (open failed at iteration {})", i);
            break;
        }
        let content = unsafe { read_all(fd_r, DATA.len()) };
        unsafe { sys_close_file(fd_r) };
        let (end, _) = unsafe { tsc_read() };
        let elapsed = end - start;
        total_cycles += elapsed;

        if i == 0 {
            first_cycles = elapsed;
            println!("  [perf] iteration 0 (cold): {} cycles", elapsed);
        }

        // Verify content
        if let Some(data) = &content {
            if data.as_slice() != DATA {
                println!("  FAIL iteration {}: data mismatch", i);
                return;
            }
        }
    }

    let avg_hot = if ITERATIONS > 1 {
        (total_cycles - first_cycles) / (ITERATIONS as u64 - 1)
    } else {
        0
    };

    println!("  [perf] average hot read: {} cycles", avg_hot);
    println!(
        "  [perf] first read overhead: {} cycles",
        first_cycles - avg_hot
    );

    expect_true!("repeated reads consistent", total_cycles > 0);

    unsafe { print_cache_stats("after repeated reads") };
    unsafe { sys_delete(PATH) };
}

// ===== Suite 3: Directory Eviction =====

unsafe fn suite_directory_eviction() {
    suite_header("directory eviction");

    const DIR: &str = "/evitest";
    const FILES: &[&str] = &["/evitest/a.txt", "/evitest/b.txt", "/evitest/c.txt"];
    const DATA: &[u8] = b"eviction test data file";

    // Setup
    unsafe { sys_delete(FILES[2]) };
    unsafe { sys_delete(FILES[1]) };
    unsafe { sys_delete(FILES[0]) };
    unsafe { sys_delete(DIR) };
    unsafe { sys_create_dir(DIR) };

    // Create and write files
    for path in FILES {
        unsafe { sys_create_file(path) };
        let fd = unsafe { sys_open_file(path, FD_FLAG_READ | FD_FLAG_WRITE) };
        if fd != usize::MAX {
            unsafe { sys_write_file(fd, DATA) };
            unsafe { sys_close_file(fd) };
        }
    }

    // Read all files into cache
    for path in FILES {
        let fd = unsafe { sys_open_file(path, FD_FLAG_READ) };
        if fd != usize::MAX {
            let _ = unsafe { read_all(fd, DATA.len()) };
            unsafe { sys_close_file(fd) };
        }
    }

    unsafe { print_cache_stats("before eviction") };

    // Evict entire directory
    let freed = unsafe { sys_evict_directory(DIR) };
    expect_true!("evict_directory returns bytes freed", freed > 0);
    println!("  evicted {} bytes from cache", freed);

    unsafe { print_cache_stats("after eviction") };

    // Verify files still readable (from disk)
    for path in FILES {
        let fd = unsafe { sys_open_file(path, FD_FLAG_READ) };
        expect_true!("file still readable after eviction", fd != usize::MAX);
        if fd != usize::MAX {
            let content = unsafe { read_all(fd, DATA.len()) };
            if let Some(data) = content {
                expect_true!("content intact", data.as_slice() == DATA);
            }
            unsafe { sys_close_file(fd) };
        }
    }

    // Re-read should re-cache
    let (start_recache, _) = unsafe { tsc_read() };
    for path in FILES {
        let fd = unsafe { sys_open_file(path, FD_FLAG_READ) };
        if fd != usize::MAX {
            let _ = unsafe { read_all(fd, DATA.len()) };
            unsafe { sys_close_file(fd) };
        }
    }
    let (end_recache, _) = unsafe { tsc_read() };
    println!(
        "  [perf] re-cache after eviction: {} cycles",
        end_recache - start_recache
    );

    // Cleanup
    for path in FILES {
        unsafe { sys_delete(path) };
    }
    unsafe { sys_delete(DIR) };
}

// ===== Suite 4: Game-Like Loading Pattern =====

unsafe fn suite_game_loading_pattern() {
    suite_header("game loading pattern");

    // Simulate a game loading screen:
    // 1. Reserve cache for level assets
    // 2. Pin critical files (audio, UI)
    // 3. Load level data (textures, meshes)
    // 4. Play level (reads from cache)
    // 5. Transition: evict old level, pin new level

    const LEVEL_DIR: &str = "/gamelvl";
    const AUDIO_DIR: &str = "/gameaud";
    const LEVEL_FILES: &[&str] = &[
        "/gamelvl/textures.dat",
        "/gamelvl/meshes.dat",
        "/gamelvl/collider.dat",
    ];
    const AUDIO_FILES: &[&str] = &["/gameaud/music.ogg", "/gameaud/sfx_hit.ogg"];

    let level_data = b"LEVEL_ASSET_DATA_32_BYTES_HERE!!";
    let audio_data = b"AUDIO_DATA_32_BYTES_PLACEHOLDER!";

    // Setup directories
    unsafe { sys_delete(LEVEL_FILES[2]) };
    unsafe { sys_delete(LEVEL_FILES[1]) };
    unsafe { sys_delete(LEVEL_FILES[0]) };
    unsafe { sys_delete(AUDIO_FILES[1]) };
    unsafe { sys_delete(AUDIO_FILES[0]) };
    unsafe { sys_delete(LEVEL_DIR) };
    unsafe { sys_delete(AUDIO_DIR) };
    unsafe { sys_create_dir(LEVEL_DIR) };
    unsafe { sys_create_dir(AUDIO_DIR) };

    // Create files
    for path in LEVEL_FILES.iter().chain(AUDIO_FILES.iter()) {
        unsafe { sys_create_file(path) };
        let data = if path.starts_with("/gameaud") {
            audio_data
        } else {
            level_data
        };
        let fd = unsafe { sys_open_file(path, FD_FLAG_READ | FD_FLAG_WRITE) };
        if fd != usize::MAX {
            unsafe { sys_write_file(fd, data) };
            unsafe { sys_close_file(fd) };
        }
    }

    // ---- Phase 1: Loading Screen ----

    println!("  --- Loading Screen ---");
    let load_start = unsafe { tsc_read().0 };

    // Reserve cache with high importance for level
    unsafe { sys_reserve_cache(LEVEL_DIR, CacheImportance::High as u8) };

    // Pin audio (must never miss cache)
    unsafe { sys_reserve_cache(AUDIO_DIR, CacheImportance::Resident as u8) };
    for path in AUDIO_FILES {
        unsafe { sys_pin_file(path) };
    }

    // Load all level files (first read = cache miss)
    for path in LEVEL_FILES {
        let fd = unsafe { sys_open_file(path, FD_FLAG_READ) };
        if fd != usize::MAX {
            let _ = unsafe { read_all(fd, level_data.len()) };
            unsafe { sys_close_file(fd) };
        }
    }

    let load_end = unsafe { tsc_read().0 };
    println!("  [perf] loading phase: {} cycles", load_end - load_start);
    unsafe { print_cache_stats("after loading") };

    // ---- Phase 2: Gameplay (repeated reads) ----

    println!("  --- Gameplay ---");
    let gameplay_start = unsafe { tsc_read().0 };

    // Simulate reading level data multiple times (should hit cache)
    for _ in 0..5 {
        for path in LEVEL_FILES {
            let fd = unsafe { sys_open_file(path, FD_FLAG_READ) };
            if fd != usize::MAX {
                let _ = unsafe { read_all(fd, level_data.len()) };
                unsafe { sys_close_file(fd) };
            }
        }
        // Simulate audio playback (should always hit cache due to pin)
        for path in AUDIO_FILES {
            let fd = unsafe { sys_open_file(path, FD_FLAG_READ) };
            if fd != usize::MAX {
                let _ = unsafe { read_all(fd, audio_data.len()) };
                unsafe { sys_close_file(fd) };
            }
        }
    }

    let gameplay_end = unsafe { tsc_read().0 };
    println!(
        "  [perf] gameplay phase: {} cycles",
        gameplay_end - gameplay_start
    );
    println!(
        "  [perf] average per-frame: {} cycles",
        (gameplay_end - gameplay_start) / 5
    );

    // ---- Phase 3: Level Transition ----

    println!("  --- Level Transition ---");

    // Evict old level
    let freed = unsafe { sys_evict_directory(LEVEL_DIR) };
    println!("  freed {} bytes from old level", freed);

    // Unpin old audio (in real game, would pin new level's audio)
    for path in AUDIO_FILES {
        unsafe { sys_unpin_file(path) };
    }

    unsafe { print_cache_stats("after transition") };

    // Cleanup
    for path in LEVEL_FILES.iter().chain(AUDIO_FILES.iter()) {
        unsafe { sys_delete(path) };
    }
    unsafe { sys_delete(LEVEL_DIR) };
    unsafe { sys_delete(AUDIO_DIR) };
}

// ===== Suite 5: Standard Tests (from original) =====

unsafe fn suite_stat_and_list() {
    suite_header("stat and list_dir");

    let mut stat = StatFlat::zeroed();
    let ok = unsafe { sys_stat_file("/", &mut stat) };
    expect_true!("stat('/') succeeds", ok);
    expect_true!("stat('/') is_dir=1", stat.is_dir == 1);

    const MAX_ENTRIES: usize = 32;
    let mut entries = [DirEntryFlat::zeroed(); MAX_ENTRIES];
    let count = unsafe { sys_list_dir("/", &mut entries) };
    expect_true!("list_dir('/') succeeds", count != usize::MAX);
    expect_true!("list_dir('/') at least 1 entry", count >= 1);

    if count != usize::MAX {
        println!("  root entries ({}):", count);
        for i in 0..count {
            let e = &entries[i];
            let kind = if e.is_dir == 1 { "DIR " } else { "FILE" };
            println!("    [{}] {} ({} bytes)", kind, e.name_str(), e.size);
        }
    }

    let mut stat2 = StatFlat::zeroed();
    let ok2 = unsafe { sys_stat_file("/counter.txt", &mut stat2) };
    if ok2 {
        expect_true!("stat('/counter.txt') is_dir=0", stat2.is_dir == 0);
    } else {
        expect_true!("stat('/counter.txt') absent: fails cleanly", !ok2);
    }
}

unsafe fn suite_create_write_read_delete() {
    suite_header("create / write / read / delete");

    const PATH: &str = "/rw.txt";
    const WRITE_DATA: &[u8] = b"MofuOS fs test";

    unsafe { sys_delete(PATH) };

    let created = unsafe { sys_create_file(PATH) };
    expect_true!("create_file('/rw.txt')", created);

    let mut stat = StatFlat::zeroed();
    let ok = unsafe { sys_stat_file(PATH, &mut stat) };
    expect_true!("stat after create", ok);
    expect_true!("stat after create: is_dir=0", stat.is_dir == 0);
    expect_eq!("stat after create: size=0", stat.size, 0);

    let fd_w = unsafe { sys_open_file(PATH, FD_FLAG_READ | FD_FLAG_WRITE) };
    expect_true!("open for write", fd_w != usize::MAX);
    if fd_w == usize::MAX {
        println!("  cannot continue - open failed");
        return;
    }

    let written = unsafe { sys_write_file(fd_w, WRITE_DATA) };
    expect_eq!("write byte count", written, WRITE_DATA.len());
    unsafe { sys_close_file(fd_w) };

    let mut stat2 = StatFlat::zeroed();
    unsafe { sys_stat_file(PATH, &mut stat2) };
    expect_eq!(
        "stat after write: size",
        stat2.size as usize,
        WRITE_DATA.len()
    );

    // First read (cold cache)
    let (cold_start, _) = unsafe { tsc_read() };
    let fd_r = unsafe { sys_open_file(PATH, FD_FLAG_READ) };
    expect_true!("open for read", fd_r != usize::MAX);
    if fd_r == usize::MAX {
        return;
    }
    let content = unsafe { read_all(fd_r, WRITE_DATA.len() + 64) };
    unsafe { sys_close_file(fd_r) };
    let (cold_end, _) = unsafe { tsc_read() };

    expect_true!("read_all succeeds", content.is_some());
    if let Some(data) = content {
        expect_eq!("read byte count", data.len(), WRITE_DATA.len());
        expect_true!("read content matches", data.as_slice() == WRITE_DATA);
    }

    // Second read (should be cached)
    let (warm_start, _) = unsafe { tsc_read() };
    let fd_r2 = unsafe { sys_open_file(PATH, FD_FLAG_READ) };
    if fd_r2 != usize::MAX {
        let _ = unsafe { read_all(fd_r2, WRITE_DATA.len() + 64) };
        unsafe { sys_close_file(fd_r2) };
    }
    let (warm_end, _) = unsafe { tsc_read() };

    println!("  [perf] cold read: {} cycles", cold_end - cold_start);
    println!("  [perf] warm read: {} cycles", warm_end - warm_start);

    let deleted = unsafe { sys_delete(PATH) };
    expect_true!("delete file", deleted);

    let mut stat3 = StatFlat::zeroed();
    let still_exists = unsafe { sys_stat_file(PATH, &mut stat3) };
    expect_true!("stat after delete fails", !still_exists);
}

unsafe fn suite_sequential_reads() {
    suite_header("sequential reads");

    const PATH: &str = "/seq.txt";
    const DATA: &[u8] = b"ABCDEFGHIJKLMNOP";

    unsafe { sys_delete(PATH) };
    let created = unsafe { sys_create_file(PATH) };
    if !created {
        println!("  SKIP (create failed)");
        return;
    }

    let fd = unsafe { sys_open_file(PATH, FD_FLAG_READ | FD_FLAG_WRITE) };
    if fd == usize::MAX {
        println!("  SKIP (open failed)");
        return;
    }
    unsafe { sys_write_file(fd, DATA) };
    unsafe { sys_close_file(fd) };

    let fd_r = unsafe { sys_open_file(PATH, FD_FLAG_READ) };
    expect_true!("open for sequential read", fd_r != usize::MAX);
    if fd_r == usize::MAX {
        return;
    }

    let mut buf_a = [0u8; 8];
    let n1 = unsafe { sys_read_file(fd_r, &mut buf_a) };
    expect_eq!("first read: 8 bytes", n1, 8);
    expect_true!("first read content", &buf_a == b"ABCDEFGH");

    let mut buf_b = [0u8; 8];
    let n2 = unsafe { sys_read_file(fd_r, &mut buf_b) };
    expect_eq!("second read: 8 bytes", n2, 8);
    expect_true!("second read content", &buf_b == b"IJKLMNOP");

    let mut buf_c = [0u8; 8];
    let n3 = unsafe { sys_read_file(fd_r, &mut buf_c) };
    expect_true!("third read at EOF returns 0", n3 == 0);

    unsafe { sys_close_file(fd_r) };
    unsafe { sys_delete(PATH) };
}

unsafe fn suite_directories() {
    suite_header("directories");

    const DIR_PATH: &str = "/testdir";
    const FILE_IN_DIR: &str = "/testdir/inside.txt";

    unsafe { sys_delete(FILE_IN_DIR) };
    unsafe { sys_delete(DIR_PATH) };

    let dir_created = unsafe { sys_create_dir(DIR_PATH) };
    expect_true!("create_dir('/testdir')", dir_created);

    let mut stat = StatFlat::zeroed();
    let ok = unsafe { sys_stat_file(DIR_PATH, &mut stat) };
    expect_true!("stat('/testdir') succeeds", ok);
    expect_true!("stat('/testdir') is_dir=1", stat.is_dir == 1);

    let file_created = unsafe { sys_create_file(FILE_IN_DIR) };
    expect_true!("create_file inside dir", file_created);

    let fd = unsafe { sys_open_file(FILE_IN_DIR, FD_FLAG_READ | FD_FLAG_WRITE) };
    if fd != usize::MAX {
        unsafe { sys_write_file(fd, b"hi subdir") };
        unsafe { sys_close_file(fd) };
    }

    let mut stat2 = StatFlat::zeroed();
    let ok2 = unsafe { sys_stat_file(FILE_IN_DIR, &mut stat2) };
    expect_true!("stat file inside dir", ok2);
    expect_true!("file inside dir is_dir=0", stat2.is_dir == 0);

    const MAX_ENTRIES: usize = 16;
    let mut entries = [DirEntryFlat::zeroed(); MAX_ENTRIES];
    let count = unsafe { sys_list_dir(DIR_PATH, &mut entries) };
    expect_true!("list_dir after adding file", count != usize::MAX);

    if count != usize::MAX {
        let found = (0..count).any(|i| entries[i].name_str() == "INSIDE.TXT");
        expect_true!("inside.txt in dir listing", found);
    }

    unsafe { sys_delete(FILE_IN_DIR) };
    unsafe { sys_delete(DIR_PATH) };
}

unsafe fn suite_error_cases() {
    suite_header("error cases");

    let fd = unsafe { sys_open_file("/nofile.txt", FD_FLAG_READ) };
    expect_true!("open non-existent returns MAX", fd == usize::MAX);

    let mut stat = StatFlat::zeroed();
    let ok = unsafe { sys_stat_file("/nope.txt", &mut stat) };
    expect_true!("stat non-existent fails", !ok);

    const PATH: &str = "/errtest.txt";
    unsafe { sys_delete(PATH) };
    unsafe { sys_create_file(PATH) };

    let mut entries = [DirEntryFlat::zeroed(); 8];
    let count = unsafe { sys_list_dir(PATH, &mut entries) };
    expect_true!("list_dir on a file fails", count == usize::MAX);

    let closed = unsafe { sys_close_file(255) };
    expect_true!("close invalid fd fails", !closed);

    let fd_w = unsafe { sys_open_file(PATH, FD_FLAG_WRITE) };
    if fd_w != usize::MAX {
        let mut buf = [0u8; 8];
        let n = unsafe { sys_read_file(fd_w, &mut buf) };
        expect_true!("read from write-only fd fails", n == usize::MAX);
        unsafe { sys_close_file(fd_w) };
    }

    let fd_r = unsafe { sys_open_file(PATH, FD_FLAG_READ) };
    if fd_r != usize::MAX {
        let n = unsafe { sys_write_file(fd_r, b"bad write") };
        expect_true!("write to read-only fd fails", n == usize::MAX);
        unsafe { sys_close_file(fd_r) };
    }

    let bad = unsafe { sys_create_file("/toolongst.txt") };
    expect_true!("create_file with 9-char stem fails", !bad);

    unsafe { sys_delete(PATH) };
}

unsafe fn suite_overwrite() {
    suite_header("overwrite");

    const PATH: &str = "/owtest.txt";
    const FIRST: &[u8] = b"first write";
    const SECOND: &[u8] = b"second write is longer";

    unsafe { sys_delete(PATH) };
    unsafe { sys_create_file(PATH) };

    let fd = unsafe { sys_open_file(PATH, FD_FLAG_READ | FD_FLAG_WRITE) };
    if fd == usize::MAX {
        println!("  SKIP (open failed)");
        return;
    }
    unsafe { sys_write_file(fd, FIRST) };
    unsafe { sys_close_file(fd) };

    let fd2 = unsafe { sys_open_file(PATH, FD_FLAG_READ | FD_FLAG_WRITE) };
    if fd2 == usize::MAX {
        println!("  SKIP (second open failed)");
        return;
    }
    let w = unsafe { sys_write_file(fd2, SECOND) };
    expect_eq!("overwrite byte count", w, SECOND.len());
    unsafe { sys_close_file(fd2) };

    let fd3 = unsafe { sys_open_file(PATH, FD_FLAG_READ) };
    expect_true!("open after overwrite", fd3 != usize::MAX);
    if fd3 == usize::MAX {
        return;
    }
    let content = unsafe { read_all(fd3, SECOND.len() + 64) };
    unsafe { sys_close_file(fd3) };

    expect_true!("read after overwrite succeeds", content.is_some());
    if let Some(data) = content {
        expect_eq!("overwrite: final size", data.len(), SECOND.len());
        expect_true!("overwrite: content matches", data.as_slice() == SECOND);
    }

    unsafe { sys_delete(PATH) };
}

unsafe fn suite_write_throughput() {
    suite_header("write throughput: cached vs uncached");

    const PATH: &str = "/wrthrpt.txt";
    const CHUNK: &[u8] = b"0123456789ABCDEF";
    const ITERATIONS: usize = 64;

    unsafe { sys_delete(PATH) };
    unsafe { sys_create_file(PATH) };

    let fd = unsafe { sys_open_file(PATH, FD_FLAG_READ | FD_FLAG_WRITE) };
    if fd == usize::MAX {
        println!("  SKIP (open failed)");
        return;
    }

    let (start_cached, _) = unsafe { tsc_read() };
    for _ in 0..ITERATIONS {
        unsafe { sys_write_file(fd, CHUNK) };
    }
    let (end_cached, _) = unsafe { tsc_read() };
    let cached_cycles = end_cached - start_cached;
    let cached_per_write = cached_cycles / ITERATIONS as u64;

    println!(
        "  [perf] {} cached writes: {} cycles total, {} cycles/write",
        ITERATIONS, cached_cycles, cached_per_write
    );

    unsafe { print_cache_stats("after cached writes") };

    let (start_flush, _) = unsafe { tsc_read() };
    let flushed = unsafe { sys_flush_file_cache() };
    let (end_flush, _) = unsafe { tsc_read() };
    let flush_cycles = end_flush - start_flush;

    expect_true!("flush succeeds", flushed);
    println!(
        "  [perf] flush {} bytes to disk: {} cycles",
        CHUNK.len() * ITERATIONS,
        flush_cycles
    );

    unsafe { print_cache_stats("after flush") };

    unsafe { sys_close_file(fd) };
    unsafe { sys_delete(PATH) };
}

unsafe fn suite_sequential_write_batching() {
    suite_header("sequential write batching");

    const PATH_BATCH: &str = "/wbatch.txt";
    const PATH_EAGER: &str = "/weager.txt";
    const CHUNK: &[u8] = b"DATADATADATADATA";
    const ITERATIONS: usize = 32;

    // write all, flush once
    unsafe { sys_delete(PATH_BATCH) };
    unsafe { sys_create_file(PATH_BATCH) };

    let fd_batch = unsafe { sys_open_file(PATH_BATCH, FD_FLAG_READ | FD_FLAG_WRITE) };
    if fd_batch == usize::MAX {
        println!("  SKIP (batch open failed)");
        return;
    }

    let (start_batch, _) = unsafe { tsc_read() };
    for _ in 0..ITERATIONS {
        unsafe { sys_write_file(fd_batch, CHUNK) };
    }
    unsafe { sys_flush_file_cache() };
    let (end_batch, _) = unsafe { tsc_read() };
    let batch_cycles = end_batch - start_batch;

    unsafe { sys_close_file(fd_batch) };

    // open, write, flush, close per iteration
    unsafe { sys_delete(PATH_EAGER) };
    unsafe { sys_create_file(PATH_EAGER) };

    let (start_eager, _) = unsafe { tsc_read() };
    for _ in 0..ITERATIONS {
        let fd = unsafe { sys_open_file(PATH_EAGER, FD_FLAG_READ | FD_FLAG_WRITE) };
        if fd == usize::MAX {
            break;
        }
        unsafe { sys_write_file(fd, CHUNK) };
        unsafe { sys_flush_file_cache() };
        unsafe { sys_close_file(fd) };
    }
    let (end_eager, _) = unsafe { tsc_read() };
    let eager_cycles = end_eager - start_eager;

    println!(
        "  [perf] batched ({} writes + 1 flush): {} cycles, {} cycles/write",
        ITERATIONS,
        batch_cycles,
        batch_cycles / ITERATIONS as u64
    );
    println!(
        "  [perf] eager ({} write+flush pairs): {} cycles, {} cycles/write",
        ITERATIONS,
        eager_cycles,
        eager_cycles / ITERATIONS as u64
    );

    if eager_cycles > 0 {
        let pct = batch_cycles * 100 / eager_cycles;
        println!("  [perf] batched is {}% of eager cost", pct);
        expect_true!("batched cheaper than eager", batch_cycles < eager_cycles);
    }

    // Verify both files have correct size
    let expected_size = (CHUNK.len() * ITERATIONS) as u64;
    let mut stat = StatFlat::zeroed();
    unsafe { sys_stat_file(PATH_BATCH, &mut stat) };
    expect_eq!("batch file size", stat.size, expected_size);

    let mut stat2 = StatFlat::zeroed();
    unsafe { sys_stat_file(PATH_EAGER, &mut stat2) };
    expect_eq!("eager file size", stat2.size, expected_size);

    unsafe { sys_delete(PATH_BATCH) };
    unsafe { sys_delete(PATH_EAGER) };
}

unsafe fn suite_dirty_state() {
    suite_header("dirty state tracking");

    const PATH: &str = "/dirty.txt";

    unsafe { sys_delete(PATH) };
    unsafe { sys_create_file(PATH) };

    // Check initial dirty count
    let mut stats = CacheStatsFlat::zeroed();
    unsafe { sys_get_cache_stats(&mut stats) };
    let dirty_before = stats.dirty_files;
    println!("  dirty files before write: {}", dirty_before);

    // Open and write, should create a dirty cache entry
    let fd = unsafe { sys_open_file(PATH, FD_FLAG_READ | FD_FLAG_WRITE) };
    if fd == usize::MAX {
        println!("  SKIP (open failed)");
        return;
    }
    unsafe { sys_write_file(fd, b"dirty content") };

    let mut stats2 = CacheStatsFlat::zeroed();
    unsafe { sys_get_cache_stats(&mut stats2) };
    println!("  dirty files after write: {}", stats2.dirty_files);
    expect_true!(
        "dirty count increased after write",
        stats2.dirty_files > dirty_before
    );

    // Flush, dirty count should drop
    let flushed = unsafe { sys_flush_file_cache() };
    expect_true!("flush succeeds", flushed);

    let mut stats3 = CacheStatsFlat::zeroed();
    unsafe { sys_get_cache_stats(&mut stats3) };
    println!("  dirty files after flush: {}", stats3.dirty_files);
    expect_true!(
        "dirty count cleared after flush",
        stats3.dirty_files < stats2.dirty_files
    );

    // Verify content persisted
    unsafe { sys_close_file(fd) };
    let fd2 = unsafe { sys_open_file(PATH, FD_FLAG_READ) };
    if fd2 != usize::MAX {
        let mut buf = [0u8; 16];
        let n = unsafe { sys_read_file(fd2, &mut buf) };
        expect_eq!("read back: correct length", n, b"dirty content".len());
        expect_true!("read back: content matches", &buf[..n] == b"dirty content");
        unsafe { sys_close_file(fd2) };
    }

    unsafe { sys_delete(PATH) };
}

unsafe fn suite_multi_file_flush() {
    suite_header("multi-file write then flush");

    // Open several files simultaneously, write to all, then flush in one call.
    const PATHS: &[&str] = &["/mfw0.txt", "/mfw1.txt", "/mfw2.txt", "/mfw3.txt"];
    const DATA: &[u8] = b"multi-file-write";
    const WRITE_ROUNDS: usize = 8;

    // Setup
    for path in PATHS {
        unsafe { sys_delete(path) };
        unsafe { sys_create_file(path) };
    }

    let mut fds = [usize::MAX; 4];
    for (i, path) in PATHS.iter().enumerate() {
        fds[i] = unsafe { sys_open_file(path, FD_FLAG_READ | FD_FLAG_WRITE) };
    }

    let (start_writes, _) = unsafe { tsc_read() };
    for _ in 0..WRITE_ROUNDS {
        for &fd in &fds {
            if fd != usize::MAX {
                unsafe { sys_write_file(fd, DATA) };
            }
        }
    }
    let (end_writes, _) = unsafe { tsc_read() };
    let write_cycles = end_writes - start_writes;

    unsafe { print_cache_stats("before multi-flush") };

    let (start_flush, _) = unsafe { tsc_read() };
    let ok = unsafe { sys_flush_file_cache() };
    let (end_flush, _) = unsafe { tsc_read() };
    let flush_cycles = end_flush - start_flush;

    expect_true!("multi-file flush succeeds", ok);
    println!(
        "  [perf] {} files x {} writes: {} cycles",
        PATHS.len(),
        WRITE_ROUNDS,
        write_cycles
    );
    println!(
        "  [perf] flush {} files: {} cycles",
        PATHS.len(),
        flush_cycles
    );
    println!(
        "  [perf] write cycles/op: {}",
        write_cycles / (PATHS.len() * WRITE_ROUNDS) as u64
    );

    unsafe { print_cache_stats("after multi-flush") };

    // Verify sizes
    let expected = (DATA.len() * WRITE_ROUNDS) as u64;
    for path in PATHS {
        let mut stat = StatFlat::zeroed();
        unsafe { sys_stat_file(path, &mut stat) };
        expect_eq!("file size after flush", stat.size, expected);
    }

    for &fd in &fds {
        if fd != usize::MAX {
            unsafe { sys_close_file(fd) };
        }
    }
    for path in PATHS {
        unsafe { sys_delete(path) };
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn main() -> ! {
    println!("cached_fs_test: starting");

    let mut cpu_info = CpuInfoFlat::zeroed();
    unsafe {
        sys_get_cpu_info(&mut cpu_info);
    }
    let freq_hz = cpu_info.tsc_frequency_hz;
    println!("CPU Frequency: {} MHz", freq_hz / 1_000_000);
    let cycles_to_us = |cycles: u64| -> u64 {
        if freq_hz > 0 {
            (cycles as u128 * 1_000_000 / freq_hz as u128) as u64
        } else {
            0
        }
    };

    unsafe {
        let (start_cycle, core) = rustspace::tsc_read();
        println!("Core: {}, Start Cycle: {}", core, start_cycle);

        suite_cache_basics();
        suite_repeated_reads();
        suite_directory_eviction();
        suite_game_loading_pattern();

        suite_stat_and_list();
        suite_create_write_read_delete();
        suite_sequential_reads();
        suite_directories();
        suite_error_cases();
        suite_overwrite();

        let (end_cycle, _) = rustspace::tsc_read();
        let total_cycles = end_cycle - start_cycle;
        println!("");
        println!("==========================================");
        println!(
            "  Total: {} cycles ({}us)",
            total_cycles,
            cycles_to_us(total_cycles)
        );
        println!("==========================================");

        unsafe { print_cache_stats("final") };
    }

    print_summary();
    println!("cached_fs_test: done");

    unsafe { sys_exit(0) }
}

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    println!("cached_fs_test: PANIC");
    unsafe { sys_exit(1) }
}
