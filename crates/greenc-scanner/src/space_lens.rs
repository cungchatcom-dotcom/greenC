//! Hierarchical SpaceNode aggregation and Bruls squarified treemap layout generator (Task T022).

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Hierarchical folder tree node representing physical disk allocation.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SpaceNode {
    pub name: String,
    pub path: PathBuf,
    pub physical_bytes: u64,
    pub file_count: u64,
    pub is_dir: bool,
    pub reason_code: Option<String>,
    pub children: Vec<SpaceNode>,
}

impl SpaceNode {
    pub fn new(path: &Path, is_dir: bool) -> Self {
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| path.display().to_string());

        Self {
            name,
            path: path.to_path_buf(),
            physical_bytes: 0,
            file_count: 0,
            is_dir,
            reason_code: classify_path_reason(path),
            children: Vec::new(),
        }
    }

    pub fn empty(path: &Path) -> Self {
        Self::new(path, true)
    }

    /// Recursively sorts children descending by physical_bytes and prunes small nodes below threshold.
    pub fn sort_and_prune(&mut self, min_bytes_threshold: u64) {
        self.children
            .sort_by_key(|b| std::cmp::Reverse(b.physical_bytes));
        for child in &mut self.children {
            child.sort_and_prune(min_bytes_threshold);
        }
        self.children
            .retain(|c| c.physical_bytes >= min_bytes_threshold);
    }
}

/// Bounding rectangle for squarified treemap visualization.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TreemapRect {
    pub name: String,
    pub path: PathBuf,
    pub physical_bytes: u64,
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
    pub depth: u8,
    pub percentage: f64,
    pub reason_code: Option<String>,
    pub children: Vec<TreemapRect>,
}

/// Squarified treemap layout generator implementing the Bruls-Huizing-van Wijk algorithm.
pub struct SquarifiedTreemapGenerator;

impl SquarifiedTreemapGenerator {
    /// Generates layout rectangles inside the given bounding box [0, 0, width, height].
    pub fn generate_layout(
        node: &SpaceNode,
        width: f64,
        height: f64,
        max_depth: u8,
    ) -> TreemapRect {
        Self::layout_recursive(node, 0.0, 0.0, width, height, 0, max_depth)
    }

    fn layout_recursive(
        node: &SpaceNode,
        x: f64,
        y: f64,
        width: f64,
        height: f64,
        depth: u8,
        max_depth: u8,
    ) -> TreemapRect {
        let mut rect = TreemapRect {
            name: node.name.clone(),
            path: node.path.clone(),
            physical_bytes: node.physical_bytes,
            x,
            y,
            width,
            height,
            depth,
            percentage: 100.0,
            reason_code: node.reason_code.clone(),
            children: Vec::new(),
        };

        if depth >= max_depth
            || node.children.is_empty()
            || node.physical_bytes == 0
            || width <= 0.0
            || height <= 0.0
        {
            return rect;
        }

        let valid_children: Vec<&SpaceNode> = node
            .children
            .iter()
            .filter(|c| c.physical_bytes > 0)
            .collect();
        if valid_children.is_empty() {
            return rect;
        }

        let total_child_bytes: u64 = valid_children.iter().map(|c| c.physical_bytes).sum();
        if total_child_bytes == 0 {
            return rect;
        }

        let child_boxes = Self::squarify(
            &valid_children,
            x,
            y,
            width,
            height,
            total_child_bytes as f64,
        );

        for (child_node, (cx, cy, cw, ch)) in valid_children.iter().zip(child_boxes) {
            let child_pct =
                (child_node.physical_bytes as f64 / node.physical_bytes.max(1) as f64) * 100.0;
            let mut child_rect =
                Self::layout_recursive(child_node, cx, cy, cw, ch, depth + 1, max_depth);
            child_rect.percentage = child_pct;
            rect.children.push(child_rect);
        }

        rect
    }

    fn squarify(
        children: &[&SpaceNode],
        x: f64,
        y: f64,
        w: f64,
        h: f64,
        total_bytes: f64,
    ) -> Vec<(f64, f64, f64, f64)> {
        let mut results = Vec::new();
        let mut remaining_box = (x, y, w, h);
        let mut row: Vec<&SpaceNode> = Vec::new();
        let total_area = w * h;

        for &child in children {
            let mut test_row = row.clone();
            test_row.push(child);

            let short_side = remaining_box.2.min(remaining_box.3);
            if row.is_empty()
                || Self::worst_aspect_ratio(&test_row, short_side, total_bytes, total_area)
                    <= Self::worst_aspect_ratio(&row, short_side, total_bytes, total_area)
            {
                row.push(child);
            } else {
                remaining_box =
                    Self::layout_row(&row, remaining_box, total_bytes, total_area, &mut results);
                row = vec![child];
            }
        }

        if !row.is_empty() {
            Self::layout_row(&row, remaining_box, total_bytes, total_area, &mut results);
        }

        results
    }

    fn worst_aspect_ratio(
        row: &[&SpaceNode],
        short_side: f64,
        total_bytes: f64,
        total_area: f64,
    ) -> f64 {
        if short_side <= 0.0 || total_bytes <= 0.0 {
            return f64::MAX;
        }

        let row_bytes: u64 = row.iter().map(|n| n.physical_bytes).sum();
        let row_area = (row_bytes as f64 / total_bytes) * total_area;
        let row_length = row_area / short_side;

        if row_length <= 0.0 {
            return f64::MAX;
        }

        let mut worst = 1.0;
        for &node in row {
            let item_area = (node.physical_bytes as f64 / total_bytes) * total_area;
            let item_side = item_area / row_length;
            if item_side <= 0.0 {
                continue;
            }
            let ratio = (row_length / item_side).max(item_side / row_length);
            if ratio > worst {
                worst = ratio;
            }
        }

        worst
    }

    fn layout_row(
        row: &[&SpaceNode],
        bounds: (f64, f64, f64, f64),
        total_bytes: f64,
        total_area: f64,
        results: &mut Vec<(f64, f64, f64, f64)>,
    ) -> (f64, f64, f64, f64) {
        let (bx, by, bw, bh) = bounds;
        let is_horizontal = bw >= bh;
        let short_side = bw.min(bh);

        let row_bytes: u64 = row.iter().map(|n| n.physical_bytes).sum();
        let row_area = (row_bytes as f64 / total_bytes) * total_area;
        let row_thickness = if short_side > 0.0 {
            row_area / short_side
        } else {
            0.0
        };

        let mut offset = 0.0;
        for &node in row {
            let item_area = (node.physical_bytes as f64 / total_bytes) * total_area;
            let item_length = if row_thickness > 0.0 {
                item_area / row_thickness
            } else {
                0.0
            };

            if is_horizontal {
                results.push((bx, by + offset, row_thickness, item_length));
            } else {
                results.push((bx + offset, by, item_length, row_thickness));
            }

            offset += item_length;
        }

        if is_horizontal {
            (bx + row_thickness, by, (bw - row_thickness).max(0.0), bh)
        } else {
            (bx, by + row_thickness, bw, (bh - row_thickness).max(0.0))
        }
    }
}

/// Path classification heuristics for disk write reason codes.
pub fn classify_path_reason(path: &Path) -> Option<String> {
    let s = path.to_string_lossy().to_lowercase();
    if s.contains("zalo")
        || s.contains("dingtalk")
        || s.contains("wechat")
        || s.contains("telegram")
    {
        Some("MEDIA_CHAT".to_string())
    } else if s.contains("temp") || s.contains("cache") || s.contains("pip") || s.contains("npm") {
        Some("CACHE_TEMP".to_string())
    } else if s.contains("crashpad") || s.contains("dumps") || s.contains("minidump") {
        Some("CRASH_DUMP".to_string())
    } else if s.contains("logs") || s.contains(".log") {
        Some("LOG_GROWTH".to_string())
    } else if s.contains("wsl") || s.contains("docker") || s.contains(".vhdx") {
        Some("CONTAINER_VM".to_string())
    } else if s.contains("appdata") || s.contains("programdata") {
        Some("APP_DATA".to_string())
    } else {
        None
    }
}
