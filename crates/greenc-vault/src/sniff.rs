//! Format sniffer and incompressible bypass heuristics (Task T028).
//!
//! Evaluates whether a file should be stored uncompressed (`ZIP_STORED`)
//! based on 47 file extensions, 15 magic byte signatures, or a 64 KiB
//! sample compression ratio probe.

use std::path::Path;

/// Compression decision for an archive entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompressionDecision {
    /// File is already compressed; store without re-compressing (ZIP method 0).
    Stored,
    /// File is compressible; compress with DEFLATE (ZIP method 8).
    Deflated,
}

impl CompressionDecision {
    /// Converts to `zip::CompressionMethod`.
    pub fn to_zip_method(self) -> zip::CompressionMethod {
        match self {
            Self::Stored => zip::CompressionMethod::Stored,
            Self::Deflated => zip::CompressionMethod::Deflated,
        }
    }
}

/// 47 incompressible file extensions across 6 domain categories.
pub const INCOMPRESSIBLE_EXTENSIONS: &[&str] = &[
    // 1. Video (9)
    "mp4", "mkv", "mov", "avi", "webm", "m4v", "3gp", "flv", "wmv", // 2. Audio (8)
    "mp3", "m4a", "aac", "ogg", "opus", "flac", "amr", "wma", // 3. Image (8)
    "jpg", "jpeg", "png", "gif", "webp", "heic", "heif", "avif", // 4. Archive (9)
    "zip", "7z", "rar", "gz", "tgz", "bz2", "xz", "zst", "lz4",
    // 5. Package & Installer (8)
    "cab", "msi", "msix", "appx", "apk", "jar", "iso", "dmg",
    // 6. Pre-compressed Document (5)
    "docx", "xlsx", "pptx", "odt", "epub",
];

/// 15 Magic Byte Signatures for extensionless cache files.
pub const COMPRESSED_MAGIC_SIGNATURES: &[(&[u8], &str)] = &[
    (&[0x1F, 0x8B], "gzip/tgz"),
    (&[0x50, 0x4B, 0x03, 0x04], "zip/jar/docx/whl/apk/nupkg"),
    (&[0x28, 0xB5, 0x2F, 0xFD], "zstd"),
    (&[0xFD, 0x37, 0x7A, 0x58, 0x5A, 0x00], "xz"),
    (&[0x37, 0x7A, 0xBC, 0xAF, 0x27, 0x1C], "7z"),
    (&[0x52, 0x61, 0x72, 0x21, 0x1A, 0x07], "rar"),
    (&[0x42, 0x5A, 0x68], "bzip2"),
    (&[0x04, 0x22, 0x4D, 0x18], "lz4_frame"),
    (b"MSCF", "cab"),
    (&[0x89, 0x50, 0x4E, 0x47], "png"),
    (&[0xFF, 0xD8, 0xFF], "jpeg"),
    (b"GIF8", "gif"),
    (b"OggS", "ogg"),
    (b"fLaC", "flac"),
    (b"wOF2", "woff2"),
];

/// Default sample window for sample compression ratio sniffing (64 KiB).
pub const SNIFF_SAMPLE_SIZE: usize = 64 * 1024;

/// Minimum deflate savings ratio (0.95 = must save at least 5% space).
pub const MIN_DEFLATE_SAVINGS_RATIO: f64 = 0.95;

/// Format Sniffer utility providing fast classification of file compressibility.
#[derive(Debug, Clone, Copy, Default)]
pub struct FormatSniffer;

impl FormatSniffer {
    /// Checks whether an extension belongs to the 47 incompressible formats catalog.
    pub fn is_incompressible_extension(ext: &str) -> bool {
        let clean = ext.trim().trim_start_matches('.').to_ascii_lowercase();
        INCOMPRESSIBLE_EXTENSIONS.iter().any(|&e| e == clean)
    }

    /// Checks whether the raw header bytes match any of the 15 magic header signatures
    /// or ISO-BMFF / RIFF WEBP containers.
    pub fn matches_magic_header(header: &[u8]) -> bool {
        if header.is_empty() {
            return false;
        }

        // 1. Direct prefix magic signatures
        for &(sig, _) in COMPRESSED_MAGIC_SIGNATURES {
            if header.len() >= sig.len() && &header[..sig.len()] == sig {
                return true;
            }
        }

        // 2. ISO-BMFF box (offset 4..8 == b"ftyp") for mp4, mov, heic, avif
        if header.len() >= 8 && &header[4..8] == b"ftyp" {
            return true;
        }

        // 3. RIFF WEBP container (offset 0..4 == b"RIFF" and 8..12 == b"WEBP")
        if header.len() >= 12 && &header[0..4] == b"RIFF" && &header[8..12] == b"WEBP" {
            return true;
        }

        false
    }

    /// Probes sample compressibility using fast level 1 compression.
    /// If compressed size is >= 95% of uncompressed size, marks as `Stored`.
    pub fn probe_compression(sample: &[u8]) -> CompressionDecision {
        if sample.is_empty() {
            return CompressionDecision::Stored;
        }

        // Fast entropy & compressibility test using level-1 compression
        match zstd::stream::encode_all(sample, 1) {
            Ok(compressed) => {
                let threshold = (sample.len() as f64 * MIN_DEFLATE_SAVINGS_RATIO) as usize;
                if compressed.len() >= threshold {
                    CompressionDecision::Stored
                } else {
                    CompressionDecision::Deflated
                }
            }
            Err(_) => CompressionDecision::Stored,
        }
    }

    /// Comprehensive compression decision heuristic combining extension,
    /// magic header inspection, and sample compression ratio probe.
    pub fn determine_compression(path: &Path, header: &[u8]) -> CompressionDecision {
        // Step 1: Check file extension
        if let Some(ext) = path.extension().and_then(|s| s.to_str()) {
            if Self::is_incompressible_extension(ext) {
                return CompressionDecision::Stored;
            }
        }

        // Step 2: Check magic header bytes
        if Self::matches_magic_header(header) {
            return CompressionDecision::Stored;
        }

        // Step 3: Probe sample if sufficient bytes are available
        if header.len() >= 512 {
            let sample_len = header.len().min(SNIFF_SAMPLE_SIZE);
            return Self::probe_compression(&header[..sample_len]);
        }

        // Default to Deflated for small unknown files
        CompressionDecision::Deflated
    }
}
