// ==============================================================================
// EMPIRICAL ADVERSARIAL CHALLENGE SUITE: FORECAST MATH & SQUARIFIED TREEMAP
// Target: C:\DiskGuard\crates\greenc-scanner\src\forecast.rs
//         C:\DiskGuard\crates\greenc-scanner\src\space_lens.rs
// Subagent: challenger_m2_2 (Milestone 2 Forecast & Treemap Challenger)
// ==============================================================================

const assert = require('assert');

// ------------------------------------------------------------------------------
// Faithful implementation of forecast.rs
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

    if (primaryBurnRate <= 0n || total === 0n) {
        days90 = null;
        daysFull = null;
    } else {
        const burnF = Number(primaryBurnRate);

        if (currentUsed >= target90) {
            days90 = 0;
        } else {
            const needed = target90 - currentUsed;
            days90 = Math.ceil(Number(needed) / burnF);
        }

        if (free === 0n) {
            daysFull = 0;
        } else {
            daysFull = Math.ceil(Number(free) / burnF);
        }
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
// Faithful implementation of space_lens.rs
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
            results.push({ x: bx, y: by + offset, width: rowThickness, height: itemLength, bytes: node.physical_bytes });
        } else {
            results.push({ x: bx + offset, y: by, width: itemLength, height: rowThickness, bytes: node.physical_bytes });
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

function classifyPathReason(pathStr) {
    const s = pathStr.toLowerCase();
    if (s.includes("zalo") || s.includes("dingtalk") || s.includes("wechat") || s.includes("telegram")) {
        return "MEDIA_CHAT";
    } else if (s.includes("temp") || s.includes("cache") || s.includes("pip") || s.includes("npm")) {
        return "CACHE_TEMP";
    } else if (s.includes("crashpad") || s.includes("dumps") || s.includes("minidump")) {
        return "CRASH_DUMP";
    } else if (s.includes("logs") || s.includes(".log")) {
        return "LOG_GROWTH";
    } else if (s.includes("wsl") || s.includes("docker") || s.includes(".vhdx")) {
        return "CONTAINER_VM";
    } else if (s.includes("appdata") || s.includes("programdata")) {
        return "APP_DATA";
    } else {
        return null;
    }
}

// ------------------------------------------------------------------------------
// TEST SUITE EXECUTION & ASSERTIONS
// ------------------------------------------------------------------------------
const results = {
    passed: 0,
    failed: 0,
    tests: []
};

function runTest(name, fn) {
    try {
        fn();
        results.passed++;
        results.tests.push({ name, status: 'PASS' });
        console.log(`[PASS] ${name}`);
    } catch (e) {
        results.failed++;
        results.tests.push({ name, status: 'FAIL', error: e.message });
        console.error(`[FAIL] ${name}: ${e.message}`);
    }
}

console.log("==============================================================================");
console.log("RUNNING EMPIRICAL ADVERSARIAL STRESS HARNESS: FORECAST & TREEMAP");
console.log("==============================================================================\n");

// --- SECTION 1: FORECAST MATH STRESS TESTS ---

runTest("FC-01: Zero net growth (velocity = 0)", () => {
    const snapshots = [
        { snapshot_date: "2026-11-01", net_change_bytes: 0 },
        { snapshot_date: "2026-11-02", net_change_bytes: 0 },
        { snapshot_date: "2026-11-03", net_change_bytes: 0 }
    ];
    // 50 GB free, 100 GB total (50% used)
    const res = predictFullness(snapshots, 50n * 1024n * 1024n * 1024n, 100n * 1024n * 1024n * 1024n);
    assert.strictEqual(res.daily_net_bps, 0n);
    assert.strictEqual(res.days_until_full, null);
    assert.strictEqual(res.days_until_90_pct, null);
    assert.strictEqual(res.is_critical, false);
});

runTest("FC-02: Negative growth (shrinking disk usage)", () => {
    const snapshots = [
        { snapshot_date: "2026-11-01", net_change_bytes: -5n * 1024n * 1024n * 1024n },
        { snapshot_date: "2026-11-02", net_change_bytes: -2n * 1024n * 1024n * 1024n }
    ];
    // 60 GB free, 100 GB total (40% used)
    const res = predictFullness(snapshots, 60n * 1024n * 1024n * 1024n, 100n * 1024n * 1024n * 1024n);
    assert(res.daily_net_bps < 0n);
    assert.strictEqual(res.days_until_full, null);
    assert.strictEqual(res.days_until_90_pct, null);
    assert.strictEqual(res.is_critical, false);
});

runTest("FC-03: Extreme rapid growth (+100 GB/day when 5 GB free)", () => {
    const snapshots = [
        { snapshot_date: "2026-11-01", net_change_bytes: 100n * 1024n * 1024n * 1024n }
    ];
    // 5 GB free, 100 GB total (95 GB used = 95%)
    const res = predictFullness(snapshots, 5n * 1024n * 1024n * 1024n, 100n * 1024n * 1024n * 1024n);
    assert.strictEqual(res.days_until_full, 1);
    assert.strictEqual(res.days_until_90_pct, 0);
    assert.strictEqual(res.is_critical, true);
});

runTest("FC-04: Exactly 90% boundary: used == 90% of total", () => {
    const snapshots = [
        { snapshot_date: "2026-11-01", net_change_bytes: 1n * 1024n * 1024n * 1024n }
    ];
    // Exactly 90% used: 10 GB free out of 100 GB
    const res = predictFullness(snapshots, 10n * 1024n * 1024n * 1024n, 100n * 1024n * 1024n * 1024n);
    assert.strictEqual(res.days_until_90_pct, 0);
    assert.strictEqual(res.is_critical, true);
});

runTest("FC-05: 100% full drive with positive growth", () => {
    const snapshots = [
        { snapshot_date: "2026-11-01", net_change_bytes: 1n * 1024n * 1024n * 1024n }
    ];
    // 0 bytes free, 100 GB total
    const res = predictFullness(snapshots, 0n, 100n * 1024n * 1024n * 1024n);
    assert.strictEqual(res.days_until_full, 0);
    assert.strictEqual(res.days_until_90_pct, 0);
    assert.strictEqual(res.is_critical, true);
});

runTest("FC-06: 100% full drive with zero growth (reveals line 70 behavior)", () => {
    const snapshots = [
        { snapshot_date: "2026-11-01", net_change_bytes: 0n }
    ];
    // 0 bytes free, 100 GB total, but velocity is 0
    const res = predictFullness(snapshots, 0n, 100n * 1024n * 1024n * 1024n);
    // In current implementation: primaryBurnRate <= 0 causes days_until_full to be null!
    console.log(`  [Insight] 100% full with velocity 0: days_until_full = ${res.days_until_full}, is_critical = ${res.is_critical}`);
    assert.strictEqual(res.is_critical, true); // Still critical due to usageRatio >= 0.90!
});

runTest("FC-07: Out-of-order date snapshots", () => {
    const s1 = { snapshot_date: "2026-11-05", net_change_bytes: 5n };
    const s2 = { snapshot_date: "2026-11-01", net_change_bytes: 1n };
    const s3 = { snapshot_date: "2026-11-03", net_change_bytes: 3n };

    const resUnordered = predictFullness([s1, s2, s3], 50n, 100n);
    const resOrdered = predictFullness([s2, s3, s1], 50n, 100n);

    assert.strictEqual(resUnordered.burn_rate_7d, resOrdered.burn_rate_7d);
    assert.strictEqual(resUnordered.burn_rate_14d, resOrdered.burn_rate_14d);
    assert.strictEqual(resUnordered.burn_rate_30d, resOrdered.burn_rate_30d);
});

runTest("FC-08: Empty snapshots", () => {
    const res = predictFullness([], 50n, 100n);
    assert.strictEqual(res.daily_net_bps, 0n);
    assert.strictEqual(res.days_until_full, null);
    assert.strictEqual(res.days_until_90_pct, null);
    assert.strictEqual(res.sample_days, 0);
});

runTest("FC-09: Division by zero safety: total_bytes = 0", () => {
    const res = predictFullness([], 0n, 0n);
    assert.strictEqual(res.usage_ratio, 0.0);
    assert.strictEqual(res.days_until_full, null);
    assert.strictEqual(res.days_until_90_pct, null);
    assert.strictEqual(res.is_critical, false);
});

runTest("FC-10: Single snapshot fallback", () => {
    const snapshots = [{ snapshot_date: "2026-11-01", net_change_bytes: 2000n }];
    const res = predictFullness(snapshots, 10000n, 20000n);
    assert.strictEqual(res.burn_rate_7d, 2000n);
    assert.strictEqual(res.burn_rate_14d, 2000n);
    assert.strictEqual(res.burn_rate_30d, 2000n);
    assert.strictEqual(res.daily_net_bps, 2000n);
});

// --- SECTION 2: SQUARIFIED TREEMAP LAYOUT STRESS TESTS ---

runTest("TM-01: Area conservation invariant - 2 children", () => {
    const root = {
        name: "root", path: "C:\\", physical_bytes: 1000, reason_code: null,
        children: [
            { name: "c1", path: "C:\\c1", physical_bytes: 600, children: [] },
            { name: "c2", path: "C:\\c2", physical_bytes: 400, children: [] }
        ]
    };
    const W = 800.0, H = 600.0;
    const layout = generateLayout(root, W, H, 2);
    const sumArea = layout.children.reduce((acc, c) => acc + c.width * c.height, 0);
    assert(Math.abs(sumArea - W * H) < 1e-6, `Sum of areas (${sumArea}) must equal W*H (${W * H})`);
});

runTest("TM-02: Single tile 100% size", () => {
    const root = {
        name: "root", path: "C:\\", physical_bytes: 500, reason_code: null,
        children: [
            { name: "solo", path: "C:\\solo", physical_bytes: 500, children: [] }
        ]
    };
    const W = 1024.0, H = 768.0;
    const layout = generateLayout(root, W, H, 2);
    assert.strictEqual(layout.children.length, 1);
    const child = layout.children[0];
    assert(Math.abs(child.width * child.height - W * H) < 1e-6);
    assert(child.x >= 0 && child.y >= 0);
    assert(child.x + child.width <= W + 1e-6);
    assert(child.y + child.height <= H + 1e-6);
});

runTest("TM-03: Degenerate input: 1,000 tiny tiles with 1 byte each", () => {
    const children = [];
    for (let i = 0; i < 1000; i++) {
        children.push({ name: `tile_${i}`, path: `C:\\tile_${i}`, physical_bytes: 1, children: [] });
    }
    const root = { name: "root", path: "C:\\", physical_bytes: 1000, reason_code: null, children };
    const W = 1920.0, H = 1080.0;
    const layout = generateLayout(root, W, H, 2);

    assert.strictEqual(layout.children.length, 1000);
    let sumArea = 0;
    for (const c of layout.children) {
        assert(!isNaN(c.x) && !isNaN(c.y) && !isNaN(c.width) && !isNaN(c.height), "No NaN coords");
        assert(isFinite(c.x) && isFinite(c.y) && isFinite(c.width) && isFinite(c.height), "No Inf coords");
        assert(c.width > 0 && c.height > 0, "Dimensions positive");
        assert(c.x >= 0 && c.y >= 0, "Non-negative origins");
        assert(c.x + c.width <= W + 1e-4, `Within W boundary: ${c.x + c.width} <= ${W}`);
        assert(c.y + c.height <= H + 1e-4, `Within H boundary: ${c.y + c.height} <= ${H}`);
        sumArea += c.width * c.height;
    }
    assert(Math.abs(sumArea - W * H) < 0.1, `Sum area ${sumArea} vs total ${W * H}`);
});

runTest("TM-04: Degenerate inputs: 0-byte items filtered safely", () => {
    const root = {
        name: "root", path: "C:\\", physical_bytes: 100, reason_code: null,
        children: [
            { name: "zero1", path: "C:\\zero1", physical_bytes: 0, children: [] },
            { name: "real", path: "C:\\real", physical_bytes: 100, children: [] },
            { name: "zero2", path: "C:\\zero2", physical_bytes: 0, children: [] }
        ]
    };
    const W = 800.0, H = 600.0;
    const layout = generateLayout(root, W, H, 2);
    // 0-byte children are filtered out so only the 100-byte child is laid out
    assert.strictEqual(layout.children.length, 1);
    assert.strictEqual(layout.children[0].name, "real");
    assert(Math.abs(layout.children[0].width * layout.children[0].height - W * H) < 1e-6);
});

runTest("TM-05: All children 0 bytes", () => {
    const root = {
        name: "root", path: "C:\\", physical_bytes: 0, reason_code: null,
        children: [
            { name: "zero1", path: "C:\\zero1", physical_bytes: 0, children: [] },
            { name: "zero2", path: "C:\\zero2", physical_bytes: 0, children: [] }
        ]
    };
    const layout = generateLayout(root, 800.0, 600.0, 2);
    assert.strictEqual(layout.children.length, 0);
});

runTest("TM-06: Zero or negative bounding dimensions", () => {
    const root = {
        name: "root", path: "C:\\", physical_bytes: 100, reason_code: null,
        children: [{ name: "c", path: "C:\\c", physical_bytes: 100, children: [] }]
    };
    const l1 = generateLayout(root, 0.0, 600.0, 2);
    assert.strictEqual(l1.children.length, 0);

    const l2 = generateLayout(root, 800.0, -10.0, 2);
    assert.strictEqual(l2.children.length, 0);
});

runTest("TM-07: Pareto/Zipfian distribution (1 massive + many tiny)", () => {
    const children = [{ name: "giant", path: "C:\\giant", physical_bytes: 900000, children: [] }];
    for (let i = 0; i < 100; i++) {
        children.push({ name: `tiny_${i}`, path: `C:\\tiny_${i}`, physical_bytes: 1000, children: [] });
    }
    const root = { name: "root", path: "C:\\", physical_bytes: 1000000, reason_code: null, children };
    const W = 1000.0, H = 1000.0;
    const layout = generateLayout(root, W, H, 2);

    assert.strictEqual(layout.children.length, 101);
    let sumArea = 0;
    for (const c of layout.children) {
        assert(!isNaN(c.width) && !isNaN(c.height));
        sumArea += c.width * c.height;
    }
    assert(Math.abs(sumArea - W * H) < 0.01);
});

runTest("TM-08: Aspect ratio bounded sanity on sorted inputs", () => {
    // Bruls squarified treemaps should avoid extreme aspect ratios on sorted inputs
    const sizes = [1000, 800, 600, 400, 300, 200, 100, 50, 20, 10];
    const children = sizes.map((s, i) => ({
        name: `item_${i}`, path: `C:\\item_${i}`, physical_bytes: s, children: []
    }));
    const total = sizes.reduce((a, b) => a + b, 0);
    const root = { name: "root", path: "C:\\", physical_bytes: total, reason_code: null, children };
    const W = 800.0, H = 600.0;
    const layout = generateLayout(root, W, H, 2);

    for (const c of layout.children) {
        const ar = Math.max(c.width / c.height, c.height / c.width);
        assert(ar < 20.0, `Aspect ratio (${ar.toFixed(2)}) is bounded for ${c.name}`);
    }
});

// --- SECTION 3: PATH REASON CLASSIFICATION STRESS TESTS ---

runTest("PC-01: Standard path classifications", () => {
    assert.strictEqual(classifyPathReason("C:\\Users\\Bob\\AppData\\Roaming\\ZaloData"), "MEDIA_CHAT");
    assert.strictEqual(classifyPathReason("C:\\DingTalk\\storage"), "MEDIA_CHAT");
    assert.strictEqual(classifyPathReason("C:\\Users\\User\\AppData\\Local\\Temp"), "CACHE_TEMP");
    assert.strictEqual(classifyPathReason("C:\\Users\\User\\.cache\\pip"), "CACHE_TEMP");
    assert.strictEqual(classifyPathReason("C:\\Crashpad\\dumps"), "CRASH_DUMP");
    assert.strictEqual(classifyPathReason("C:\\Windows\\System32\\minidump"), "CRASH_DUMP");
    assert.strictEqual(classifyPathReason("C:\\Windows\\Logs\\CBS"), "LOG_GROWTH");
    assert.strictEqual(classifyPathReason("C:\\app.log"), "LOG_GROWTH");
    assert.strictEqual(classifyPathReason("C:\\Docker\\wsl\\data.vhdx"), "CONTAINER_VM");
    assert.strictEqual(classifyPathReason("C:\\Users\\User\\AppData\\Local\\Programs"), "APP_DATA");
    assert.strictEqual(classifyPathReason("C:\\ProgramData\\Custom"), "APP_DATA");
});

runTest("PC-02: Mixed slashes and uppercase", () => {
    assert.strictEqual(classifyPathReason("c:/users/user/appdata/local/temp/foo.tmp"), "CACHE_TEMP");
    assert.strictEqual(classifyPathReason("C:\\USERS\\USER\\TELEGRAM DESKTOP"), "MEDIA_CHAT");
    assert.strictEqual(classifyPathReason("D:/docker/volumes/my_vol"), "CONTAINER_VM");
});

runTest("PC-03: Substring collision edge cases (Heuristic behavior analysis)", () => {
    // Paths containing "temp" inside other words
    const resAttempt = classifyPathReason("C:\\docs\\attempt.txt");
    const resTemplate = classifyPathReason("C:\\web\\template.html");
    console.log(`  [Heuristic Check] 'attempt.txt' classified as: ${resAttempt}`);
    console.log(`  [Heuristic Check] 'template.html' classified as: ${resTemplate}`);
    // Because .contains("temp") is substring based, these match CACHE_TEMP
    // Documenting this heuristic limitation in caveats/challenges!
    assert.strictEqual(resAttempt, "CACHE_TEMP");
    assert.strictEqual(resTemplate, "CACHE_TEMP");
});

runTest("PC-04: Non-matching arbitrary paths", () => {
    assert.strictEqual(classifyPathReason("C:\\Photos\\Vacation2026\\img01.jpg"), null);
    assert.strictEqual(classifyPathReason("D:\\Music\\album.flac"), null);
    assert.strictEqual(classifyPathReason(""), null);
});

// --- SUMMARY REPORT ---
console.log("\n==============================================================================");
console.log(`TEST SUMMARY: ${results.passed} PASSED, ${results.failed} FAILED across ${results.tests.length} tests`);
console.log("==============================================================================");

if (results.failed > 0) {
    process.exit(1);
} else {
    process.exit(0);
}
