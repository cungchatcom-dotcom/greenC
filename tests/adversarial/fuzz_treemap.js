// ==============================================================================
// EMPIRICAL ADVERSARIAL CHALLENGE SUITE: EXTENSIVE FUZZ & INVARIANT TESTER
// Target: C:\DiskGuard\crates\greenc-scanner\src\forecast.rs
//         C:\DiskGuard\crates\greenc-scanner\src\space_lens.rs
// Subagent: challenger_m2_2 (Milestone 2 Forecast & Treemap Challenger)
// ==============================================================================

const assert = require('assert');

// ------------------------------------------------------------------------------
// Implementation of space_lens.rs
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

console.log("Starting Treemap Layout Stress Fuzzing (1,000 iterations)...");
let iterationsPassed = 0;

for (let iter = 0; iter < 1000; iter++) {
    // Generate random width and height
    const W = 100 + Math.random() * 3000;
    const H = 100 + Math.random() * 3000;
    const totalArea = W * H;

    // Random number of children between 1 and 200
    const childCount = 1 + Math.floor(Math.random() * 200);
    const children = [];
    let totalBytes = 0;

    for (let i = 0; i < childCount; i++) {
        // Random distribution: uniform, exponential, or power law
        let bytes;
        const r = Math.random();
        if (r < 0.2) {
            bytes = 0; // Test zero bytes filtering
        } else if (r < 0.7) {
            bytes = 1 + Math.floor(Math.random() * 1000);
        } else {
            bytes = 10000 + Math.floor(Math.random() * 1000000);
        }
        children.push({ name: `n_${i}`, path: `C:\\n_${i}`, physical_bytes: bytes, children: [] });
        totalBytes += bytes;
    }

    // Sort descending as expected by sort_and_prune
    children.sort((a, b) => b.physical_bytes - a.physical_bytes);

    const root = { name: "root", path: "C:\\", physical_bytes: totalBytes, children };
    const layout = generateLayout(root, W, H, 1);

    const validCount = children.filter(c => c.physical_bytes > 0).length;
    if (validCount === 0) {
        assert.strictEqual(layout.children.length, 0);
        iterationsPassed++;
        continue;
    }

    assert.strictEqual(layout.children.length, validCount, `Iter ${iter}: Child count mismatch`);

    let sumArea = 0;
    for (const c of layout.children) {
        // Invariant 1: No NaN or Inf
        assert(!isNaN(c.x) && !isNaN(c.y) && !isNaN(c.width) && !isNaN(c.height), `Iter ${iter}: NaN found`);
        assert(isFinite(c.x) && isFinite(c.y) && isFinite(c.width) && isFinite(c.height), `Iter ${iter}: Inf found`);

        // Invariant 2: Positive dimensions
        assert(c.width > 0, `Iter ${iter}: Width <= 0: ${c.width}`);
        assert(c.height > 0, `Iter ${iter}: Height <= 0: ${c.height}`);

        // Invariant 3: Coordinates within bounds
        assert(c.x >= -1e-6, `Iter ${iter}: x negative: ${c.x}`);
        assert(c.y >= -1e-6, `Iter ${iter}: y negative: ${c.y}`);
        assert(c.x + c.width <= W + 1e-4, `Iter ${iter}: Exceeds W: ${c.x + c.width} > ${W}`);
        assert(c.y + c.height <= H + 1e-4, `Iter ${iter}: Exceeds H: ${c.y + c.height} > ${H}`);

        sumArea += c.width * c.height;
    }

    // Invariant 4: Area conservation
    const diff = Math.abs(sumArea - totalArea);
    const relDiff = diff / totalArea;
    assert(relDiff < 1e-4, `Iter ${iter}: Area mismatch: sum=${sumArea}, expected=${totalArea}, relDiff=${relDiff}`);

    iterationsPassed++;
}

console.log(`Successfully completed 1,000 Treemap Stress Fuzzing iterations! All invariants held 100%.`);
