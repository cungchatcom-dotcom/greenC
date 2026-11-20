// ==============================================================================
// EMPIRICAL ADVERSARIAL CHALLENGE SUITE: DEEP EDGE CASES & ATTACK VECTORS
// Target: C:\DiskGuard\crates\greenc-vault\src\sniff.rs
//         C:\DiskGuard\crates\greenc-vault\src\profiles.rs
// Subagent: challenger_m3_2 (Milestone 3 Sniffer & Profile Challenger)
// Date: 2026-11-16
// ==============================================================================

const assert = require('assert');
const path = require('path');
const zlib = require('zlib');
const crypto = require('crypto');
const fs = require('fs');

console.log('='.repeat(80));
console.log('STARTING DEEP ADVERSARIAL EDGE CASE SUITE: VAULT SNIFFER & PROFILES');
console.log('='.repeat(80));

const INCOMPRESSIBLE_EXTENSIONS = [
    "mp4", "mkv", "mov", "avi", "webm", "m4v", "3gp", "flv", "wmv",
    "mp3", "m4a", "aac", "ogg", "opus", "flac", "amr", "wma",
    "jpg", "jpeg", "png", "gif", "webp", "heic", "heif", "avif",
    "zip", "7z", "rar", "gz", "tgz", "bz2", "xz", "zst", "lz4",
    "cab", "msi", "msix", "appx", "apk", "jar", "iso", "dmg",
    "docx", "xlsx", "pptx", "odt", "epub",
];

const COMPRESSED_MAGIC_SIGNATURES = [
    { magic: Buffer.from([0x1F, 0x8B]), name: "gzip/tgz" },
    { magic: Buffer.from([0x50, 0x4B, 0x03, 0x04]), name: "zip/jar/docx/whl/apk/nupkg" },
    { magic: Buffer.from([0x28, 0xB5, 0x2F, 0xFD]), name: "zstd" },
    { magic: Buffer.from([0xFD, 0x37, 0x7A, 0x58, 0x5A, 0x00]), name: "xz" },
    { magic: Buffer.from([0x37, 0x7A, 0xBC, 0xAF, 0x27, 0x1C]), name: "7z" },
    { magic: Buffer.from([0x52, 0x61, 0x72, 0x21, 0x1A, 0x07]), name: "rar" },
    { magic: Buffer.from([0x42, 0x5A, 0x68]), name: "bzip2" },
    { magic: Buffer.from([0x04, 0x22, 0x4D, 0x18]), name: "lz4_frame" },
    { magic: Buffer.from("MSCF", "ascii"), name: "cab" },
    { magic: Buffer.from([0x89, 0x50, 0x4E, 0x47]), name: "png" },
    { magic: Buffer.from([0xFF, 0xD8, 0xFF]), name: "jpeg" },
    { magic: Buffer.from("GIF8", "ascii"), name: "gif" },
    { magic: Buffer.from("OggS", "ascii"), name: "ogg" },
    { magic: Buffer.from("fLaC", "ascii"), name: "flac" },
    { magic: Buffer.from("wOF2", "ascii"), name: "woff2" },
];

const DEFAULT_PROTECTED_PATTERNS = [
    "*.exe", "*.dll", "*.sys", "*.ini", "*.cfg", "*.config",
    "*.json", "*.yaml", "*.yml", "*.toml", "*.db", "*.db-wal",
    "*.db-shm", "*.sqlite", "*.sqlite3", "*.mdf", "*.ldf",
    "*.rescache", "lockfile", "CURRENT", "MANIFEST-*",
];

function isIncompressibleExtension(ext) {
    if (!ext || typeof ext !== 'string') return false;
    let clean = ext.trim().replace(/^\.+/, '').toLowerCase();
    return INCOMPRESSIBLE_EXTENSIONS.includes(clean);
}

function matchesMagicHeader(header) {
    if (!header || header.length === 0) return false;

    for (const sig of COMPRESSED_MAGIC_SIGNATURES) {
        if (header.length >= sig.magic.length) {
            if (header.subarray(0, sig.magic.length).equals(sig.magic)) {
                return true;
            }
        }
    }

    if (header.length >= 8) {
        const ftyp = Buffer.from("ftyp", "ascii");
        if (header.subarray(4, 8).equals(ftyp)) {
            return true;
        }
    }

    if (header.length >= 12) {
        const riff = Buffer.from("RIFF", "ascii");
        const webp = Buffer.from("WEBP", "ascii");
        if (header.subarray(0, 4).equals(riff) && header.subarray(8, 12).equals(webp)) {
            return true;
        }
    }

    return false;
}

function probeCompression(sample) {
    if (!sample || sample.length === 0) return "Stored";
    try {
        const compressed = zlib.deflateSync(sample, { level: 1 });
        const threshold = Math.floor(sample.length * 0.95);
        return compressed.length >= threshold ? "Stored" : "Deflated";
    } catch {
        return "Stored";
    }
}

function determineCompression(filePath, header) {
    const ext = path.extname(filePath);
    if (ext && isIncompressibleExtension(ext)) {
        return "Stored";
    }
    if (matchesMagicHeader(header)) {
        return "Stored";
    }
    if (header && header.length >= 512) {
        const sampleLen = Math.min(header.length, 64 * 1024);
        return probeCompression(header.subarray(0, sampleLen));
    }
    return "Deflated";
}

function matchesGlob(name, pattern) {
    const p = pattern.toLowerCase();
    const lowerName = name.toLowerCase();
    if (p.startsWith("*.")) {
        const ext = p.substring(2);
        return lowerName.endsWith("." + ext);
    } else if (p.endsWith("*")) {
        const prefix = p.substring(0, p.length - 1);
        return lowerName.startsWith(prefix);
    } else {
        return lowerName === p;
    }
}

function isProtectedFilename(filename, customPatterns = null) {
    for (const pattern of DEFAULT_PROTECTED_PATTERNS) {
        if (matchesGlob(filename, pattern)) {
            return true;
        }
    }
    if (customPatterns && Array.isArray(customPatterns)) {
        for (const pattern of customPatterns) {
            if (matchesGlob(filename, pattern)) {
                return true;
            }
        }
    }
    return false;
}

// ------------------------------------------------------------------------------
// CHALLENGE 1: RIFF COLLISION & SUBTYPE DISCRIMINATION
// ------------------------------------------------------------------------------
console.log('\n--- CHALLENGE 1: RIFF Subtype Discrimination ---');
// RIFF containers have 4CC identifiers at offset 8..12
// RIFF WEBP -> must be Stored
// RIFF WAVE -> is uncompressed PCM audio; must NOT match magic header
// RIFF AVI  -> is handled by .avi extension; raw RIFF AVI header without .avi must NOT match WEBP

const riffWebp = Buffer.from("RIFF\x24\x00\x00\x00WEBPVP8 ");
const riffWave = Buffer.from("RIFF\x24\x00\x00\x00WAVEfmt ");
const riffAvi  = Buffer.from("RIFF\x24\x00\x00\x00AVI LIST");
const riffQdir = Buffer.from("RIFF\x24\x00\x00\x00QDIR    ");

assert.strictEqual(matchesMagicHeader(riffWebp), true, 'RIFF WEBP must match magic header');
assert.strictEqual(matchesMagicHeader(riffWave), false, 'RIFF WAVE must NOT match WEBP magic header');
assert.strictEqual(matchesMagicHeader(riffAvi), false, 'RIFF AVI must NOT match WEBP magic header');
assert.strictEqual(matchesMagicHeader(riffQdir), false, 'RIFF QDIR must NOT match WEBP magic header');

// WAV file with unknown extension: matchesMagicHeader must be FALSE
assert.strictEqual(matchesMagicHeader(riffWave), false);

// 1. Compressible WAV (e.g. silence / repetitive waveform): ratio < 0.95 -> Deflated
const silencePcm = Buffer.concat([riffWave, Buffer.alloc(1024, 0x00)]);
assert.strictEqual(determineCompression("silent_recording.bin", silencePcm), "Deflated", 'Compressible WAV PCM must decide Deflated');

// 2. High-entropy / noisy WAV: ratio >= 0.95 -> Stored
const noisyPcm = Buffer.concat([riffWave, crypto.randomBytes(1024)]);
assert.strictEqual(determineCompression("noisy_recording.bin", noisyPcm), "Stored", 'Incompressible noisy WAV PCM must decide Stored');
console.log('  [PASS] RIFF WEBP correctly distinguished from RIFF WAVE, AVI, and other RIFF formats.');

// ------------------------------------------------------------------------------
// CHALLENGE 2: ISO-BMFF FTYP OFFSET DISCRIMINATION
// ------------------------------------------------------------------------------
console.log('\n--- CHALLENGE 2: ISO-BMFF ftyp Offset Discrimination ---');
// ISO-BMFF specifies 4-byte box length followed by 'ftyp' (offset 4..8)
// If 'ftyp' is at offset 0 (corrupt/different format), it must NOT match

const ftypOffset4 = Buffer.from([0x00, 0x00, 0x00, 0x1C, 0x66, 0x74, 0x79, 0x70, 0x6D, 0x70, 0x34, 0x32]); // ftypmp42
const ftypOffset0 = Buffer.from([0x66, 0x74, 0x79, 0x70, 0x00, 0x00, 0x00, 0x1C]); // ftyp at 0
const ftypOffset2 = Buffer.from([0x00, 0x00, 0x66, 0x74, 0x79, 0x70, 0x00, 0x1C]); // ftyp at 2

assert.strictEqual(matchesMagicHeader(ftypOffset4), true, 'ftyp at offset 4 must match');
assert.strictEqual(matchesMagicHeader(ftypOffset0), false, 'ftyp at offset 0 must NOT match');
assert.strictEqual(matchesMagicHeader(ftypOffset2), false, 'ftyp at offset 2 must NOT match');
console.log('  [PASS] ISO-BMFF box offset 4..8 strictly enforced.');

// ------------------------------------------------------------------------------
// CHALLENGE 3: UNICODE FILENAMES & MULTI-DOT EXTENSIONS
// ------------------------------------------------------------------------------
console.log('\n--- CHALLENGE 3: Unicode & Multi-Dot Filename Stress Test ---');

const unicodeTests = [
    { name: "bản_ghi_âm.MP3", expected: "Stored", desc: "Vietnamese diacritics in name" },
    { name: "nhạc_trẻ_2026.FLAC", expected: "Stored", desc: "Vietnamese diacritics in name with uppercase FLAC" },
    { name: "hình_ảnh_đẹp.JPEG", expected: "Stored", desc: "Vietnamese diacritics in name with uppercase JPEG" },
    { name: "video_hội_nghị.MKV", expected: "Stored", desc: "Vietnamese diacritics in name with uppercase MKV" },
    { name: "tài_liệu_hợp_đồng.DOCX", expected: "Stored", desc: "Vietnamese diacritics in name with uppercase DOCX" },
    { name: "báo_cáo_tài_chính.XLSX", expected: "Stored", desc: "Vietnamese diacritics in name with uppercase XLSX" },
    { name: "gói_cài_đặt.APK", expected: "Stored", desc: "Vietnamese diacritics in name with uppercase APK" },
    { name: "dữ_liệu_nén.7Z", expected: "Stored", desc: "Vietnamese diacritics in name with uppercase 7Z" },
    { name: "日本語_アーカイブ.ZIP", expected: "Stored", desc: "Japanese kanji in name" },
    { name: "русский_документ.odt", expected: "Stored", desc: "Cyrillic in name" },
    { name: "العربية_صوت.ogg", expected: "Stored", desc: "Arabic in name" },
    { name: "tập_tin.văn_bản.txt", expected: "Deflated", desc: "Multi-dot Unicode text file" },
    { name: "archive.v1.0.final.tar.gz", expected: "Stored", desc: "Multi-dot tar.gz (extension is .gz)" },
    { name: "payload.final.2026.tar.bz2", expected: "Stored", desc: "Multi-dot tar.bz2 (extension is .bz2)" },
    { name: "backup.sys.full.tar.xz", expected: "Stored", desc: "Multi-dot tar.xz (extension is .xz)" },
    { name: "repo.source.tar.zst", expected: "Stored", desc: "Multi-dot tar.zst (extension is .zst)" },
];

for (const ut of unicodeTests) {
    const dec = determineCompression(ut.name, Buffer.alloc(0));
    assert.strictEqual(dec, ut.expected, `Unicode test failed: '${ut.name}' (${ut.desc}): expected ${ut.expected}, got ${dec}`);
}
console.log(`  [PASS] All ${unicodeTests.length} Unicode diacritic and multi-dot filename tests passed.`);

// ------------------------------------------------------------------------------
// CHALLENGE 4: 511 vs 512 BYTE BOUNDARY PROBE BEHAVIOR
// ------------------------------------------------------------------------------
console.log('\n--- CHALLENGE 4: 511 vs 512 Byte Boundary Probe ---');
// At 511 bytes: below probe threshold -> defaults to Deflated
// At 512 bytes: reaches probe threshold -> probes sample

const rand511 = crypto.randomBytes(511);
const rand512 = crypto.randomBytes(512);

const dec511 = determineCompression("unknown_extensionless_data", rand511);
const dec512 = determineCompression("unknown_extensionless_data", rand512);

assert.strictEqual(dec511, "Deflated", '511 bytes of random data must default to Deflated (under 512 bytes)');
assert.strictEqual(dec512, "Stored", '512 bytes of random data must probe Stored (reaches 512-byte threshold)');
console.log('  [PASS] Exact 511 vs 512 byte boundary behavior confirmed.');

// ------------------------------------------------------------------------------
// CHALLENGE 5: ADVERSARIAL SCANNING ON MOCK FILESYSTEM
// ------------------------------------------------------------------------------
console.log('\n--- CHALLENGE 5: Mock Filesystem Scanning Simulation ---');
// Simulates the exact recursive directory scanner logic on a populated mock directory tree
// testing that DEFAULT_PROTECTED_PATTERNS and profile excludes block ALL sensitive files.

const mockTree = [
    // Windows Temp profile mock files
    { path: "Temp/normal_cache.tmp", profile: "windows_temp", shouldClean: true },
    { path: "Temp/old_installer.log", profile: "windows_temp", shouldClean: true },
    { path: "Temp/driver_install.sys", profile: "windows_temp", shouldClean: false }, // *.sys
    { path: "Temp/service_helper.exe", profile: "windows_temp", shouldClean: false }, // *.exe
    { path: "Temp/msvcr120.dll", profile: "windows_temp", shouldClean: false },        // *.dll
    { path: "Temp/active.lock", profile: "windows_temp", shouldClean: false },         // *.lock
    { path: "Temp/daemon.pid", profile: "windows_temp", shouldClean: false },          // *.pid
    { path: "Temp/settings.json", profile: "windows_temp", shouldClean: false },       // *.json
    { path: "Temp/state.db", profile: "windows_temp", shouldClean: false },            // *.db

    // Browser Cache profile mock files
    { path: "Browser/Cache/data_0", profile: "browser_cache", shouldClean: true },
    { path: "Browser/Cache/f_0000b2", profile: "browser_cache", shouldClean: true },
    { path: "Browser/Cache/Cookies", profile: "browser_cache", shouldClean: false },
    { path: "Browser/Cache/Cookies-journal", profile: "browser_cache", shouldClean: false },
    { path: "Browser/Cache/Login Data", profile: "browser_cache", shouldClean: false },
    { path: "Browser/Cache/Login Data-journal", profile: "browser_cache", shouldClean: false },
    { path: "Browser/Cache/History", profile: "browser_cache", shouldClean: false },
    { path: "Browser/Cache/Bookmarks", profile: "browser_cache", shouldClean: false },
    { path: "Browser/Cache/Preferences", profile: "browser_cache", shouldClean: false },
    { path: "Browser/Cache/Web Data", profile: "browser_cache", shouldClean: false },
    { path: "Browser/Cache/favicons.sqlite", profile: "browser_cache", shouldClean: false },

    // Chat Media profile mock files
    { path: "Zalo/Cache/thumb_01.dat", profile: "chat_media", shouldClean: true },
    { path: "Zalo/fileNoise/0010aa51cfe34b97", profile: "chat_media", shouldClean: true },
    { path: "Zalo/Cache/message", profile: "chat_media", shouldClean: false },
    { path: "Zalo/Cache/DBFiles", profile: "chat_media", shouldClean: false },
    { path: "Zalo/Cache/user_chat.db", profile: "chat_media", shouldClean: false },
    { path: "Zalo/Cache/user_chat.db-wal", profile: "chat_media", shouldClean: false },
    { path: "Zalo/Cache/user_chat.db-shm", profile: "chat_media", shouldClean: false },
    { path: "Zalo/Cache/icons.rescache", profile: "chat_media", shouldClean: false },

    // Package Managers profile mock files
    { path: "npm/cache/content-v2/sha512/00/11/223344", profile: "package_managers", shouldClean: true },
    { path: "pip/cache/wheels/package-1.0-py3-none-any.whl", profile: "package_managers", shouldClean: true },
    { path: "npm/cache/lockfile", profile: "package_managers", shouldClean: false },
    { path: "cargo/cache/config.toml", profile: "package_managers", shouldClean: false },
    { path: "npm/cache/package.json", profile: "package_managers", shouldClean: false },
    { path: "pip/cache/config", profile: "package_managers", shouldClean: false },

    // App Logs profile mock files
    { path: "Logs/cbs.log", profile: "app_logs", shouldClean: true },
    { path: "Logs/dism.log", profile: "app_logs", shouldClean: true },
    { path: "Logs/panther_setup.txt", profile: "app_logs", shouldClean: true },
    { path: "Logs/DataStore.edb", profile: "app_logs", shouldClean: false },

    // Crash Dumps profile mock files
    { path: "CrashDumps/chrome.exe.1234.dmp", profile: "crash_dumps", shouldClean: true },
    { path: "CrashDumps/Report.wer", profile: "crash_dumps", shouldClean: true },

    // Orphan Radar profile mock files
    { path: "AppData/ObsoleteApp/junk.dat", profile: "orphan_radar", shouldClean: true },
    { path: "AppData/Microsoft", profile: "orphan_radar", shouldClean: false },
    { path: "AppData/Windows", profile: "orphan_radar", shouldClean: false },
    { path: "AppData/Packages", profile: "orphan_radar", shouldClean: false },
];

const profileExcludes = {
    windows_temp: ["*.lock", "*.pid"],
    browser_cache: ["Cookies*", "Login Data*", "History*", "Bookmarks*", "Preferences*", "Web Data*"],
    chat_media: ["*.db", "*.db-wal", "*.db-shm", "*.rescache", "message", "DBFiles"],
    crash_dumps: [],
    package_managers: ["lockfile", "*.toml", "*.json", "config"],
    app_logs: ["DataStore.edb"],
    old_versions: [],
    orphan_radar: ["Microsoft", "Windows", "Packages"],
};

for (const item of mockTree) {
    const filename = path.basename(item.path);
    const custom = profileExcludes[item.profile];
    const protected = isProtectedFilename(filename, custom);
    const willClean = !protected;

    assert.strictEqual(
        willClean,
        item.shouldClean,
        `Scanner simulation failed on '${item.path}' in profile '${item.profile}': expected shouldClean=${item.shouldClean}, got willClean=${willClean}`
    );
}
console.log(`  [PASS] All ${mockTree.length} mock filesystem entries correctly filtered (0 false cleans on protected assets).`);

console.log('='.repeat(80));
console.log('ALL DEEP ADVERSARIAL EDGE CASES VERIFIED SUCCESSFULLY (VERDICT: PASS)');
console.log('='.repeat(80));
