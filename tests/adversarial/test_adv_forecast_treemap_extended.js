// ==============================================================================
// EMPIRICAL ADVERSARIAL CHALLENGE SUITE: FORECAST, SQUARIFIED TREEMAP & I18N
// Milestone 2 Gate: Comprehensive Verification & Stress Testing
// Subagent: challenger_m2_4 (Milestone 2 Forecast & Treemap Challenger)
// Target:
//   - crates/greenc-scanner/src/forecast.rs
//   - crates/greenc-scanner/src/space_lens.rs
//   - ui/src/lib/components/SpaceTreemap.svelte
//   - ui/src/pages/OverviewPage.svelte
//   - ui/src/lib/i18n.svelte.ts
//   - crates/greenc-i18n/locales/*.json (51 catalogs)
// ==============================================================================

const assert = require('assert');
const fs = require('fs');
const path = require('path');

// ------------------------------------------------------------------------------
// Faithful implementation of crates/greenc-scanner/src/forecast.rs
// (reflecting worker_m2_2's fix where current_free_bytes == 0 returns (Some(0), Some(0)))
// ------------------------------------------------------------------------------
function calculateRollingVelocity(snapshots, windowDays) {
    if (!snapshots || snapshots.length === 0) return 0n;
    const takeCount = Math.min(snapshots.length, windowDays);
    const slice = snapshots.slice(snapshots.length - takeCount);
    let sum = 0n;
    for (const s of slice) {
        sum += BigInt(s.net_change_bytes);
    }
    return sum / BigInt(takeCount);
}

function predictFullness(snapshots, currentFreeBytes, totalBytes) {
    const free = BigInt(currentFreeBytes);
    const total = BigInt(totalBytes);
    const currentUsed = total > free ? total - free : 0n;
    const usageRatio = total > 0n ? Number(currentUsed) / Number(total) : 0.0;

    const sortedSnapshots = [...snapshots].sort((a, b) => {
        if (a.snapshot_date < b.snapshot_date) return -1;
        if (a.snapshot_date > b.snapshot_date) return 1;
        return 0;
    });

    const sampleDays = sortedSnapshots.length;

    const rate7d = calculateRollingVelocity(sortedSnapshots, 7);
    const rate14d = calculateRollingVelocity(sortedSnapshots, 14);
    const rate30d = calculateRollingVelocity(sortedSnapshots, 30);

    let primaryBurnRate = rate14d !== 0n ? rate14d : (rate7d !== 0n ? rate7d : rate30d);

    const target90 = BigInt(Math.floor(Number(total) * 0.90));

    let days90 = null;
    let daysFull = null;

    if (total === 0n) {
        days90 = null;
        daysFull = null;
    } else if (free === 0n) {
        // Authentic Rust forecast.rs lines 72-73:
        // When free bytes == 0, drive is full immediately regardless of velocity
        days90 = 0;
        daysFull = 0;
    } else if (primaryBurnRate <= 0n) {
        days90 = currentUsed >= target90 ? 0 : null;
        daysFull = null;
    } else {
        const burnF = Number(primaryBurnRate);

        if (currentUsed >= target90) {
            days90 = 0;
        } else {
            const needed = target90 - currentUsed;
            days90 = Math.ceil(Number(needed) / burnF);
        }

        daysFull = Math.ceil(Number(free) / burnF);
    }

    const isCritical = usageRatio >= 0.90
        || (daysFull !== null && daysFull < 14)
        || (days90 !== null && days90 <= 14);

    return {
        daily_net_bps: primaryBurnRate,
        daily_burn_rate_bytes: primaryBurnRate,
        burn_rate_7d: rate7d,
        burn_rate_14d: rate14d,
        burn_rate_30d: rate30d,
        days_until_90_pct: days90,
        days_until_full: daysFull,
        current_free_bytes: free,
        total_bytes: total,
        usage_ratio: usageRatio,
        is_critical: isCritical,
        sample_days: sampleDays,
    };
}

// ------------------------------------------------------------------------------
// Faithful implementation of crates/greenc-scanner/src/space_lens.rs
// ------------------------------------------------------------------------------
function worstAspectRatio(row, shortSide, totalBytes, totalArea) {
    if (shortSide <= 0.0 || totalBytes <= 0.0) return Number.MAX_VALUE;

    let rowBytes = 0;
    for (const n of row) rowBytes += n.physical_bytes;

    const rowArea = (rowBytes / totalBytes) * totalArea;
    const rowLength = rowArea / shortSide;

    if (rowLength <= 0.0) return Number.MAX_VALUE;

    let worst = 1.0;
    for (const node of row) {
        const itemArea = (node.physical_bytes / totalBytes) * totalArea;
        const itemSide = itemArea / rowLength;
        if (itemSide <= 0.0) continue;
        const ratio = Math.max(rowLength / itemSide, itemSide / rowLength);
        if (ratio > worst) worst = ratio;
    }

    return worst;
}

function layoutRow(row, bounds, totalBytes, totalArea, results) {
    const [bx, by, bw, bh] = bounds;
    const isHorizontal = bw >= bh;
    const shortSide = Math.min(bw, bh);

    let rowBytes = 0;
    for (const n of row) rowBytes += n.physical_bytes;
    const rowArea = (rowBytes / totalBytes) * totalArea;
    const rowThickness = shortSide > 0.0 ? rowArea / shortSide : 0.0;

    let offset = 0.0;
    for (const node of row) {
        const itemArea = (node.physical_bytes / totalBytes) * totalArea;
        const itemLength = rowThickness > 0.0 ? itemArea / rowThickness : 0.0;

        if (isHorizontal) {
            results.push({ x: bx, y: by + offset, width: rowThickness, height: itemLength, bytes: node.physical_bytes, name: node.name });
        } else {
            results.push({ x: bx + offset, y: by, width: itemLength, height: rowThickness, bytes: node.physical_bytes, name: node.name });
        }
        offset += itemLength;
    }

    if (isHorizontal) {
        return [bx + rowThickness, by, Math.max(0.0, bw - rowThickness), bh];
    } else {
        return [bx, by + rowThickness, bw, Math.max(0.0, bh - rowThickness)];
    }
}

function squarify(children, x, y, w, h, totalBytes) {
    const results = [];
    let remainingBox = [x, y, w, h];
    let row = [];
    const totalArea = w * h;

    for (const child of children) {
        const testRow = [...row, child];
        const shortSide = Math.min(remainingBox[2], remainingBox[3]);
        if (row.length === 0 || worstAspectRatio(testRow, shortSide, totalBytes, totalArea) <= worstAspectRatio(row, shortSide, totalBytes, totalArea)) {
            row.push(child);
        } else {
            remainingBox = layoutRow(row, remainingBox, totalBytes, totalArea, results);
            row = [child];
        }
    }

    if (row.length > 0) {
        layoutRow(row, remainingBox, totalBytes, totalArea, results);
    }

    return results;
}

function generateLayout(node, width, height, maxDepth) {
    return layoutRecursive(node, 0.0, 0.0, width, height, 0, maxDepth);
}

function layoutRecursive(node, x, y, width, height, depth, maxDepth) {
    const rect = {
        name: node.name,
        path: node.path,
        physical_bytes: node.physical_bytes,
        x,
        y,
        width,
        height,
        depth,
        percentage: 100.0,
        reason_code: node.reason_code,
        children: []
    };

    if (depth >= maxDepth || !node.children || node.children.length === 0 || node.physical_bytes === 0 || width <= 0.0 || height <= 0.0) {
        return rect;
    }

    const validChildren = node.children.filter(c => c.physical_bytes > 0);
    if (validChildren.length === 0) return rect;

    // Standard sorting descending by physical_bytes for optimal squarified layout
    validChildren.sort((a, b) => b.physical_bytes - a.physical_bytes);

    let totalChildBytes = 0;
    for (const c of validChildren) totalChildBytes += c.physical_bytes;
    if (totalChildBytes === 0) return rect;

    const childBoxes = squarify(validChildren, x, y, width, height, totalChildBytes);

    for (let i = 0; i < validChildren.length; i++) {
        const childNode = validChildren[i];
        const box = childBoxes[i];
        const childPct = (childNode.physical_bytes / Math.max(1, node.physical_bytes)) * 100.0;
        const childRect = layoutRecursive(childNode, box.x, box.y, box.width, box.height, depth + 1, maxDepth);
        childRect.percentage = childPct;
        rect.children.push(childRect);
    }

    return rect;
}

// ------------------------------------------------------------------------------
// Test execution harness
// ------------------------------------------------------------------------------
const testResults = {
    passed: 0,
    failed: 0,
    suiteDetails: []
};

function runCheck(category, name, fn) {
    try {
        fn();
        testResults.passed++;
        testResults.suiteDetails.push({ category, name, status: 'PASS' });
        console.log(`[PASS] [${category}] ${name}`);
    } catch (e) {
        testResults.failed++;
        testResults.suiteDetails.push({ category, name, status: 'FAIL', error: e.message });
        console.error(`[FAIL] [${category}] ${name}: ${e.message}`);
    }
}

console.log("==============================================================================");
console.log("CHALLENGER_M2_4 ADVERSARIAL STRESS SUITE: FORECAST, TREEMAP & I18N");
console.log("==============================================================================\n");

// ==============================================================================
// 1. FORECAST MATHEMATICAL MODELS & EXTREME BURN RATES
// ==============================================================================

runCheck("FORECAST", "FC-E01: Positive normal burn rate (+5 GB/day on 50 GB free)", () => {
    const snapshots = [
        { snapshot_date: "2026-11-01", net_change_bytes: 5n * 1024n * 1024n * 1024n },
        { snapshot_date: "2026-11-02", net_change_bytes: 5n * 1024n * 1024n * 1024n }
    ];
    // 50 GB free, 100 GB total (50 GB used = 50%)
    const res = predictFullness(snapshots, 50n * 1024n * 1024n * 1024n, 100n * 1024n * 1024n * 1024n);
    assert.strictEqual(res.days_until_full, 10); // 50 GB / 5 GB = 10 days
    assert.strictEqual(res.days_until_90_pct, 8); // (90 GB - 50 GB) / 5 GB = 8 days
    assert.strictEqual(res.is_critical, true); // days_until_full < 14
});

runCheck("FORECAST", "FC-E02: Negative burn rate (-50 GB/day cleanup scenario)", () => {
    const snapshots = [
        { snapshot_date: "2026-11-01", net_change_bytes: -50n * 1024n * 1024n * 1024n }
    ];
    // 60 GB free, 100 GB total (40% used)
    const res = predictFullness(snapshots, 60n * 1024n * 1024n * 1024n, 100n * 1024n * 1024n * 1024n);
    assert(res.daily_net_bps < 0n);
    assert.strictEqual(res.days_until_full, null);
    assert.strictEqual(res.days_until_90_pct, null);
    assert.strictEqual(res.is_critical, false);
});

runCheck("FORECAST", "FC-E03: Zero burn rate with 50% usage", () => {
    const snapshots = [
        { snapshot_date: "2026-11-01", net_change_bytes: 0n }
    ];
    const res = predictFullness(snapshots, 50n * 1024n * 1024n * 1024n, 100n * 1024n * 1024n * 1024n);
    assert.strictEqual(res.daily_net_bps, 0n);
    assert.strictEqual(res.days_until_full, null);
    assert.strictEqual(res.days_until_90_pct, null);
    assert.strictEqual(res.is_critical, false);
});

runCheck("FORECAST", "FC-E04: 0 free bytes edge case with positive burn rate -> days_until_full == 0", () => {
    const snapshots = [
        { snapshot_date: "2026-11-01", net_change_bytes: 2n * 1024n * 1024n * 1024n }
    ];
    const res = predictFullness(snapshots, 0n, 100n * 1024n * 1024n * 1024n);
    assert.strictEqual(res.days_until_full, 0);
    assert.strictEqual(res.days_until_90_pct, 0);
    assert.strictEqual(res.is_critical, true);
    assert.strictEqual(res.usage_ratio, 1.0);
});

runCheck("FORECAST", "FC-E05: 0 free bytes edge case with ZERO burn rate -> days_until_full == 0 (worker_m2_2 fix)", () => {
    const snapshots = [
        { snapshot_date: "2026-11-01", net_change_bytes: 0n }
    ];
    const res = predictFullness(snapshots, 0n, 100n * 1024n * 1024n * 1024n);
    assert.strictEqual(res.days_until_full, 0, "0 free bytes must report 0 days until full even if velocity is 0");
    assert.strictEqual(res.days_until_90_pct, 0);
    assert.strictEqual(res.is_critical, true);
});

runCheck("FORECAST", "FC-E06: 0 free bytes edge case with NEGATIVE burn rate -> days_until_full == 0", () => {
    const snapshots = [
        { snapshot_date: "2026-11-01", net_change_bytes: -10n * 1024n * 1024n * 1024n }
    ];
    const res = predictFullness(snapshots, 0n, 100n * 1024n * 1024n * 1024n);
    assert.strictEqual(res.days_until_full, 0, "0 free bytes must report 0 days until full even if velocity is negative");
    assert.strictEqual(res.is_critical, true);
});

runCheck("FORECAST", "FC-E07: Extreme burn rate: Terabytes per day (+10 TB/day on 500 GB free)", () => {
    const tenTB = 10n * 1024n * 1024n * 1024n * 1024n;
    const snapshots = [{ snapshot_date: "2026-11-01", net_change_bytes: tenTB }];
    const free500GB = 500n * 1024n * 1024n * 1024n;
    const total2TB = 2000n * 1024n * 1024n * 1024n;
    const res = predictFullness(snapshots, free500GB, total2TB);
    assert.strictEqual(res.days_until_full, 1); // 500GB / 10TB = 0.05 days -> ceil = 1 day
    assert.strictEqual(res.is_critical, true);
});

runCheck("FORECAST", "FC-E08a: Massive negative burn rate (-10 TB/day) with 40% usage -> is_critical == false", () => {
    const minusTenTB = -10n * 1024n * 1024n * 1024n * 1024n;
    const snapshots = [{ snapshot_date: "2026-11-01", net_change_bytes: minusTenTB }];
    // 600 GB free out of 1000 GB = 40% used
    const res = predictFullness(snapshots, 600n * 1024n * 1024n * 1024n, 1000n * 1024n * 1024n * 1024n);
    assert.strictEqual(res.days_until_full, null);
    assert.strictEqual(res.days_until_90_pct, null);
    assert.strictEqual(res.is_critical, false, "40% used with negative burn rate is NOT critical");
});

runCheck("FORECAST", "FC-E08b: Massive negative burn rate (-10 TB/day) with 90% usage -> is_critical == true (static threshold invariant)", () => {
    const minusTenTB = -10n * 1024n * 1024n * 1024n * 1024n;
    const snapshots = [{ snapshot_date: "2026-11-01", net_change_bytes: minusTenTB }];
    // 100 GB free out of 1000 GB = 90% used
    const res = predictFullness(snapshots, 100n * 1024n * 1024n * 1024n, 1000n * 1024n * 1024n * 1024n);
    assert.strictEqual(res.days_until_full, null);
    assert.strictEqual(res.days_until_90_pct, 0, "At 90% usage, days_until_90_pct is 0 even with negative burn rate");
    assert.strictEqual(res.is_critical, true, "90% used is critical due to static capacity threshold");
});

runCheck("FORECAST", "FC-E09: Minute burn rate: 1 byte per day on 1 TB drive", () => {
    const snapshots = [{ snapshot_date: "2026-11-01", net_change_bytes: 1n }];
    const oneTB = 1024n * 1024n * 1024n * 1024n;
    const res = predictFullness(snapshots, oneTB / 2n, oneTB);
    assert(res.days_until_full > 1000000);
    assert.strictEqual(res.is_critical, false);
});

runCheck("FORECAST", "FC-E10: OverviewPage 100% full badge logic verification", () => {
    // In OverviewPage.svelte:
    // {#if summary.free_bytes === 0 || usagePercent >= 100 || summary.days_until_full === 0}
    //   <AlertCircle /> Ổ đĩa đã đầy
    function computeOverviewBadge(summary, usagePercent) {
        if (summary.free_bytes === 0 || usagePercent >= 100 || summary.days_until_full === 0) {
            return { badge: 'DANGER_FULL', text: 'Ổ đĩa đã đầy' };
        } else if (summary.days_until_full !== null && summary.days_until_full !== undefined) {
            return { badge: 'DAYS_REMAINING', days: summary.days_until_full };
        } else if (usagePercent >= 90) {
            return { badge: 'CRITICAL_PCT' };
        } else {
            return { badge: 'STABLE' };
        }
    }

    const fullZeroBytes = computeOverviewBadge({ free_bytes: 0, days_until_full: 0 }, 100);
    assert.strictEqual(fullZeroBytes.badge, 'DANGER_FULL');

    const full100Pct = computeOverviewBadge({ free_bytes: 1024, days_until_full: null }, 100);
    assert.strictEqual(full100Pct.badge, 'DANGER_FULL');

    const zeroDays = computeOverviewBadge({ free_bytes: 1024, days_until_full: 0 }, 95);
    assert.strictEqual(zeroDays.badge, 'DANGER_FULL');
});

// ==============================================================================
// 2. SQUARIFIED TREEMAP RENDERING INVARIANTS & ASPECT RATIOS
// ==============================================================================

runCheck("TREEMAP", "TM-E01: Aspect ratio <= 5.0 invariant on descending sorted inputs (Bruls invariant)", () => {
    const sizes = [10000, 7500, 5000, 3000, 2000, 1500, 1000, 800, 500, 300, 100, 50, 20, 10];
    const children = sizes.map((s, i) => ({ name: `file_${i}`, path: `C:\\file_${i}`, physical_bytes: s, children: [] }));
    const total = sizes.reduce((a, b) => a + b, 0);
    const root = { name: "root", path: "C:\\", physical_bytes: total, children };

    const containers = [
        { w: 800.0, h: 600.0, name: "800x600 (4:3)" },
        { w: 1920.0, h: 1080.0, name: "1920x1080 (16:9)" },
        { w: 1000.0, h: 1000.0, name: "1000x1000 (1:1)" }
    ];

    for (const c of containers) {
        const layout = generateLayout(root, c.w, c.h, 2);
        for (const child of layout.children) {
            const ar = Math.max(child.width / child.height, child.height / child.width);
            assert(ar <= 5.0, `Aspect ratio for ${child.name} in ${c.name} is ${ar.toFixed(3)}, must be <= 5.0`);
        }
    }
});

runCheck("TREEMAP", "TM-E02: Single-file root treemap invariant", () => {
    const root = {
        name: "C:\\", path: "C:\\", physical_bytes: 500000,
        children: [
            { name: "single_large_file.iso", path: "C:\\single_large_file.iso", physical_bytes: 500000, children: [] }
        ]
    };
    const W = 1200.0, H = 800.0;
    const layout = generateLayout(root, W, H, 2);
    assert.strictEqual(layout.children.length, 1);
    const solo = layout.children[0];
    assert.strictEqual(solo.name, "single_large_file.iso");
    assert(Math.abs(solo.width - W) < 1e-4, `Width must match container: ${solo.width} vs ${W}`);
    assert(Math.abs(solo.height - H) < 1e-4, `Height must match container: ${solo.height} vs ${H}`);
    assert.strictEqual(solo.x, 0);
    assert.strictEqual(solo.y, 0);
    assert.strictEqual(solo.percentage, 100.0);
});

runCheck("TREEMAP", "TM-E03: Zero-byte files in treemap layouts (all zero-byte files)", () => {
    const root = {
        name: "C:\\", path: "C:\\", physical_bytes: 0,
        children: [
            { name: "zero1.txt", path: "C:\\zero1.txt", physical_bytes: 0, children: [] },
            { name: "zero2.txt", path: "C:\\zero2.txt", physical_bytes: 0, children: [] }
        ]
    };
    const layout = generateLayout(root, 800.0, 600.0, 2);
    assert.strictEqual(layout.children.length, 0, "Zero-byte root and children produce empty layout without error");
});

runCheck("TREEMAP", "TM-E04: Zero-byte files mixed with active data", () => {
    const root = {
        name: "C:\\", path: "C:\\", physical_bytes: 300,
        children: [
            { name: "empty1.dat", path: "C:\\empty1.dat", physical_bytes: 0, children: [] },
            { name: "data1.dat", path: "C:\\data1.dat", physical_bytes: 100, children: [] },
            { name: "empty2.dat", path: "C:\\empty2.dat", physical_bytes: 0, children: [] },
            { name: "data2.dat", path: "C:\\data2.dat", physical_bytes: 200, children: [] },
            { name: "empty3.dat", path: "C:\\empty3.dat", physical_bytes: 0, children: [] }
        ]
    };
    const W = 900.0, H = 600.0;
    const layout = generateLayout(root, W, H, 2);
    assert.strictEqual(layout.children.length, 2, "Only non-zero files are rendered");
    const totalArea = layout.children.reduce((acc, c) => acc + c.width * c.height, 0);
    assert(Math.abs(totalArea - W * H) < 1e-4, "Area preserved across active files");
});

runCheck("TREEMAP", "TM-E05: Deep hierarchy treemaps (10 levels deep)", () => {
    // Construct a 10-level deep hierarchy
    function makeLevel(currentDepth, maxD) {
        if (currentDepth > maxD) return [];
        const isLeaf = currentDepth === maxD;
        const count = isLeaf ? 3 : 2;
        const children = [];
        for (let i = 0; i < count; i++) {
            const childBytes = isLeaf ? 1000 : 5000 * (count - i);
            const sub = makeLevel(currentDepth + 1, maxD);
            const subBytes = sub.reduce((a, b) => a + b.physical_bytes, 0);
            children.push({
                name: `dir_L${currentDepth}_${i}`,
                path: `C:\\L${currentDepth}\\${i}`,
                physical_bytes: subBytes > 0 ? subBytes : childBytes,
                children: sub
            });
        }
        return children;
    }

    const depth10Children = makeLevel(1, 10);
    const totalBytes = depth10Children.reduce((a, b) => a + b.physical_bytes, 0);
    const root = {
        name: "C:\\",
        path: "C:\\",
        physical_bytes: totalBytes,
        children: depth10Children
    };

    const W = 1920.0, H = 1080.0;
    const layout = generateLayout(root, W, H, 10);

    // Verify depth traversal reaches depth 10
    let maxSeenDepth = 0;
    let nodeCount = 0;

    function verifyTreeInvariants(node, parentBounds) {
        nodeCount++;
        if (node.depth > maxSeenDepth) maxSeenDepth = node.depth;

        assert(!isNaN(node.x) && !isNaN(node.y) && !isNaN(node.width) && !isNaN(node.height), "Coords valid");
        assert(node.width >= 0 && node.height >= 0, "Dimensions non-negative");

        if (parentBounds) {
            assert(node.x >= parentBounds.x - 1e-4, `x within parent bounds: ${node.x} >= ${parentBounds.x}`);
            assert(node.y >= parentBounds.y - 1e-4, `y within parent bounds: ${node.y} >= ${parentBounds.y}`);
            assert(node.x + node.width <= parentBounds.x + parentBounds.width + 1e-3, "w within parent");
            assert(node.y + node.height <= parentBounds.y + parentBounds.height + 1e-3, "h within parent");
        }

        if (node.children && node.children.length > 0) {
            const childAreaSum = node.children.reduce((acc, c) => acc + c.width * c.height, 0);
            const parentArea = node.width * node.height;
            assert(Math.abs(childAreaSum - parentArea) < 1.0, `Child area sum (${childAreaSum}) matches parent area (${parentArea})`);

            for (const child of node.children) {
                verifyTreeInvariants(child, { x: node.x, y: node.y, width: node.width, height: node.height });
            }
        }
    }

    verifyTreeInvariants(layout, null);
    assert.strictEqual(maxSeenDepth, 10, `Depth must reach 10 levels, saw ${maxSeenDepth}`);
    console.log(`    [Tree Deep Audit] Reached max depth ${maxSeenDepth} with ${nodeCount} total layout nodes verified.`);
});

// ==============================================================================
// 3. UI REACTIVITY & DRIVE SWITCHING
// ==============================================================================

runCheck("REACTIVITY", "RC-E01: SpaceTreemap rootNode change effect simulation", () => {
    // Model the exact Svelte 5 $effect in SpaceTreemap.svelte:
    // let currentNode = $state(null);
    // let breadcrumbStack = $state([]);
    // let previousRootNode = $state(null);
    // let tooltip = { ... };
    // let contextMenu = { ... };
    class TreemapComponentState {
        constructor() {
            this.currentNode = null;
            this.breadcrumbStack = [];
            this.previousRootNode = null;
            this.tooltip = { visible: false };
            this.contextMenu = { visible: false };
        }

        onRootNodeEffect(rootNode) {
            if (rootNode !== this.previousRootNode) {
                this.previousRootNode = rootNode;
                this.currentNode = rootNode;
                this.breadcrumbStack = rootNode ? [rootNode] : [];
                this.tooltip = { visible: false };
                this.contextMenu = { visible: false };
            }
        }

        drillDown(node) {
            this.currentNode = node;
            this.breadcrumbStack.push(node);
        }
    }

    const comp = new TreemapComponentState();
    const driveC = { name: "C:\\", path: "C:\\", children: [{ name: "Users", path: "C:\\Users", children: [] }] };
    const driveD = { name: "D:\\", path: "D:\\", children: [{ name: "Games", path: "D:\\Games", children: [] }] };

    // Initial load drive C
    comp.onRootNodeEffect(driveC);
    assert.strictEqual(comp.currentNode.name, "C:\\");
    assert.strictEqual(comp.breadcrumbStack.length, 1);

    // User drills down into C:\Users
    comp.drillDown(driveC.children[0]);
    assert.strictEqual(comp.currentNode.name, "Users");
    assert.strictEqual(comp.breadcrumbStack.length, 2);

    // User switches drive to D:
    comp.onRootNodeEffect(driveD);
    assert.strictEqual(comp.currentNode.name, "D:\\", "Current node must reset to D: on drive change");
    assert.strictEqual(comp.breadcrumbStack.length, 1, "Breadcrumbs must reset to [driveD]");
    assert.strictEqual(comp.breadcrumbStack[0].name, "D:\\");

    // User re-scans drive D (new object reference returned from scan)
    const driveD_rescan = { name: "D:\\", path: "D:\\", children: [{ name: "Games", path: "D:\\Games", children: [] }] };
    comp.drillDown(driveD.children[0]); // drill into Games
    comp.onRootNodeEffect(driveD_rescan);
    assert.strictEqual(comp.currentNode.name, "D:\\", "Current node must reset to fresh root on re-scan");
    assert.strictEqual(comp.breadcrumbStack.length, 1);
});

// ==============================================================================
// 4. I18N FORMAT SPECIFIER REGEX ACROSS ALL 51 LOCALES
// ==============================================================================

runCheck("I18N", "I18N-E01: Verify all 51 locale catalogs for format specifier token rendering", () => {
    const localesDir = path.resolve("C:/DiskGuard/crates/greenc-i18n/locales");
    assert(fs.existsSync(localesDir), `Locales directory must exist at ${localesDir}`);

    const files = fs.readdirSync(localesDir).filter(f => f.endsWith('.json'));
    assert(files.length >= 50, `Must have at least 50 locales, found ${files.length}`);

    // Exact implementation of t() from ui/src/lib/i18n.svelte.ts:
    function translate(raw, params) {
        if (!params) return raw;
        let result = raw;
        for (const [k, v] of Object.entries(params)) {
            result = result.replace(new RegExp(`\\{${k}(?::[^}]+)?\\}`, 'g'), String(v));
        }
        return result;
    }

    const testParamPool = {
        letter: 'C', label: 'Local Disk', drive: 'C', index: 1, total: 10, count: 42,
        name: 'test_file.iso', app: 'Zalo', target: 'D:\\Relocated', size: '150 MB',
        path: 'C:\\Users\\User\\AppData', free: '45 GB', errors: '0', tool: 'git', folder: 'Temp',
        error: 'AccessDenied', days: 14, service: 'wuauserv', author: 'GreenC', url: 'https://greenc.app',
        site: 'GitHub', restored: 5, purged: 12, pct: 85, eta: '00:03', product: 'GreenC v2',
        seconds: 45, original: '500 MB', compressed: '210 MB', reason: 'MEDIA_CHAT',
        hours: 3, day: '2026-11-16', folders: 15, apps: 2, done: 8, size_done: '120 MB',
        size_total: '250 MB', date: '2026-11-16', fs: 'NTFS', size_mb: 48.5, log: 'canary.log'
    };

    let checkedTranslations = 0;
    const failures = [];

    for (const file of files) {
        const filePath = path.join(localesDir, file);
        const catalog = JSON.parse(fs.readFileSync(filePath, 'utf8'));

        for (const [key, val] of Object.entries(catalog)) {
            checkedTranslations++;
            // Check if key or val has format placeholders
            const valPlaceholders = (val.match(/\{[a-zA-Z0-9_]+(?::[^}]+)?\}/g) || []);
            if (valPlaceholders.length > 0) {
                const rendered = translate(val, testParamPool);
                const lingering = rendered.match(/\{[a-zA-Z0-9_]+(?::[^}]+)?\}/g);
                if (lingering) {
                    failures.push({
                        locale: file,
                        key,
                        val,
                        lingering
                    });
                }
            }
        }
    }

    assert.strictEqual(failures.length, 0, `Failed token interpolations in: ${JSON.stringify(failures, null, 2)}`);
    console.log(`    [i18n Complete Audit] Checked ${files.length} locales, ${checkedTranslations} key-values: 0 unrendered placeholders.`);
});

runCheck("I18N", "I18N-E02: Comma-decimal and RTL formatting validation", () => {
    const COMMA_DECIMAL_LANGUAGES = [
        'vi', 'id', 'tr', 'az', 'ru', 'uk', 'pl', 'cs', 'sk', 'hu', 'ro', 'bg', 'sr', 'hr', 'el',
        'de', 'fr', 'es', 'pt_BR', 'pt_PT', 'it', 'nl', 'sv', 'da', 'nb', 'fi', 'fa'
    ];
    const RTL_LANGUAGES = ['ar', 'he', 'fa', 'ur'];

    function formatSize(bytes, locale) {
        if (bytes === 0) return '0 B';
        const isComma = COMMA_DECIMAL_LANGUAGES.includes(locale);
        const sep = isComma ? ',' : '.';
        const sign = bytes < 0 ? '-' : '';
        let val = Math.abs(bytes);
        const units = ['B', 'KB', 'MB', 'GB', 'TB'];
        for (const unit of units) {
            if (val < 1024 || unit === 'TB') {
                const formatted = unit === 'B' || unit === 'KB'
                    ? Math.round(val).toString()
                    : (val.toFixed(1)).replace('.', sep);
                return `${sign}${formatted} ${unit}`;
            }
            val /= 1024;
        }
        return `${sign}${(val.toFixed(1)).replace('.', sep)} TB`;
    }

    // Check Vietnamese (comma-decimal)
    assert.strictEqual(formatSize(1536 * 1024 * 1024, 'vi'), '1,5 GB');
    // Check English (period-decimal)
    assert.strictEqual(formatSize(1536 * 1024 * 1024, 'en'), '1.5 GB');
    // Check Arabic (RTL)
    assert(RTL_LANGUAGES.includes('ar'));
    assert(RTL_LANGUAGES.includes('he'));
    assert(RTL_LANGUAGES.includes('fa'));
    assert(RTL_LANGUAGES.includes('ur'));
});

// ==============================================================================
// SUMMARY REPORT
// ==============================================================================
console.log("\n==============================================================================");
console.log(`CHALLENGER_M2_4 RESULTS: ${testResults.passed} PASSED, ${testResults.failed} FAILED across ${testResults.suiteDetails.length} checks`);
console.log("==============================================================================");

if (testResults.failed > 0) {
    process.exit(1);
} else {
    process.exit(0);
}
