<!-- ui/src/lib/components/SpaceTreemap.svelte -->
<script lang="ts">
  import { t, formatSize } from '$lib/i18n.svelte';
  import { revealInExplorer, type SpaceNode } from '$lib/api';
  import { Folder, File, ChevronRight, ExternalLink, Copy, Check } from 'lucide-svelte';

  interface Props {
    rootNode: SpaceNode | null;
    isLoading?: boolean;
    onRefreshRequested?: () => void;
  }

  let { rootNode, isLoading = false, onRefreshRequested }: Props = $props();

  // Navigation State
  let currentNode = $state<SpaceNode | null>(null);
  let breadcrumbStack = $state<SpaceNode[]>([]);
  let previousRootNode = $state<SpaceNode | null>(null);

  // Synchronize state when rootNode changes (drive switch, re-scan, or reset)
  $effect(() => {
    if (rootNode !== previousRootNode) {
      previousRootNode = rootNode;
      currentNode = rootNode;
      breadcrumbStack = rootNode ? [rootNode] : [];
      // Clear transient overlays
      tooltip = { x: 0, y: 0, visible: false, node: null };
      contextMenu = { x: 0, y: 0, visible: false, node: null };
    }
  });

  // Tooltip & Context Menu States
  let tooltip = $state<{ x: number; y: number; visible: boolean; node: SpaceNode | null }>({
    x: 0,
    y: 0,
    visible: false,
    node: null
  });

  let contextMenu = $state<{ x: number; y: number; visible: boolean; node: SpaceNode | null }>({
    x: 0,
    y: 0,
    visible: false,
    node: null
  });

  let copiedToast = $state(false);

  // Rectangle layout item
  interface TreemapRect {
    node: SpaceNode;
    x: number;
    y: number;
    w: number;
    h: number;
    color: string;
  }

  // Squarified Treemap Generator
  function computeTreemapLayout(
    node: SpaceNode | null,
    containerW: number,
    containerH: number
  ): TreemapRect[] {
    if (!node || !node.children || node.children.length === 0 || containerW <= 0 || containerH <= 0) {
      return [];
    }

    const items = [...node.children].filter((c) => c.physical_bytes > 0);
    const totalBytes = items.reduce((sum, item) => sum + item.physical_bytes, 0);
    if (totalBytes === 0) return [];

    const rects: TreemapRect[] = [];
    squarify(items, [], { x: 0, y: 0, w: containerW, h: containerH }, totalBytes, rects);
    return rects;
  }

  function squarify(
    children: SpaceNode[],
    row: SpaceNode[],
    box: { x: number; y: number; w: number; h: number },
    totalBytes: number,
    result: TreemapRect[]
  ) {
    if (children.length === 0) {
      layoutRow(row, box, totalBytes, result);
      return;
    }

    const c = children[0];
    const newRow = [...row, c];

    if (row.length === 0 || worstAspectRatio(row, box, totalBytes) >= worstAspectRatio(newRow, box, totalBytes)) {
      squarify(children.slice(1), newRow, box, totalBytes, result);
    } else {
      const remainingBox = layoutRow(row, box, totalBytes, result);
      squarify(children, [], remainingBox, totalBytes, result);
    }
  }

  function worstAspectRatio(
    row: SpaceNode[],
    box: { x: number; y: number; w: number; h: number },
    totalBytes: number
  ): number {
    const s = Math.min(box.w, box.h);
    if (s <= 0) return Infinity;

    const rowBytes = row.reduce((sum, item) => sum + item.physical_bytes, 0);
    const rowArea = (rowBytes / totalBytes) * (box.w * box.h);
    const side = rowArea / s;
    if (side <= 0) return Infinity;

    let worst = 1;
    for (const item of row) {
      const itemArea = (item.physical_bytes / totalBytes) * (box.w * box.h);
      const itemLen = itemArea / side;
      const ratio = Math.max(side / itemLen, itemLen / side);
      if (ratio > worst) worst = ratio;
    }
    return worst;
  }

  function layoutRow(
    row: SpaceNode[],
    box: { x: number; y: number; w: number; h: number },
    totalBytes: number,
    result: TreemapRect[]
  ): { x: number; y: number; w: number; h: number } {
    const isHorizontal = box.w >= box.h;
    const shortSide = Math.min(box.w, box.h);
    const rowBytes = row.reduce((sum, item) => sum + item.physical_bytes, 0);
    const rowArea = (rowBytes / totalBytes) * (box.w * box.h);
    const rowThickness = shortSide > 0 ? rowArea / shortSide : 0;

    let offset = 0;
    for (const item of row) {
      const itemArea = (item.physical_bytes / totalBytes) * (box.w * box.h);
      const itemLength = rowThickness > 0 ? itemArea / rowThickness : 0;

      const rect: TreemapRect = isHorizontal
        ? {
            node: item,
            x: box.x,
            y: box.y + offset,
            w: Math.max(1, rowThickness),
            h: Math.max(1, itemLength),
            color: getNodeColor(item)
          }
        : {
            node: item,
            x: box.x + offset,
            y: box.y,
            w: Math.max(1, itemLength),
            h: Math.max(1, rowThickness),
            color: getNodeColor(item)
          };

      result.push(rect);
      offset += itemLength;
    }

    return isHorizontal
      ? { x: box.x + rowThickness, y: box.y, w: Math.max(0, box.w - rowThickness), h: box.h }
      : { x: box.x, y: box.y + rowThickness, w: box.w, h: Math.max(0, box.h - rowThickness) };
  }

  function getNodeColor(node: SpaceNode): string {
    if (node.reason_code) {
      switch (node.reason_code) {
        case 'MEDIA_CHAT': return '#3b82f6';
        case 'CACHE_TEMP': return '#f59e0b';
        case 'EAPP_BUNDLE': return '#8b5cf6';
        case 'CONTAINER_VM': return '#06b6d4';
        case 'APP_DATA': return '#64748b';
        case 'CRASH_DUMP': return '#ef4444';
        case 'LOG_GROWTH': return '#ec4899';
      }
    }
    return node.is_dir ? '#107c41' : '#14b8a6';
  }

  let containerWidth = $state(800);
  let containerHeight = $state(420);
  let layoutRects = $derived(computeTreemapLayout(currentNode, containerWidth, containerHeight));

  function handleDrillDown(node: SpaceNode) {
    if (node.is_dir && node.children && node.children.length > 0) {
      currentNode = node;
      breadcrumbStack = [...breadcrumbStack, node];
      contextMenu.visible = false;
    }
  }

  function handleBreadcrumbClick(idx: number) {
    breadcrumbStack = breadcrumbStack.slice(0, idx + 1);
    currentNode = breadcrumbStack[idx];
  }

  function handleContextMenu(event: MouseEvent, node: SpaceNode) {
    event.preventDefault();
    contextMenu = {
      x: event.clientX,
      y: event.clientY,
      visible: true,
      node
    };
  }

  async function copyPath(path: string) {
    try {
      await navigator.clipboard.writeText(path);
      copiedToast = true;
      setTimeout(() => (copiedToast = false), 1200);
      contextMenu.visible = false;
    } catch {}
  }
</script>

<svelte:window onclick={() => (contextMenu.visible = false)} />

<div class="flex flex-col gap-3 w-full rounded-xl border border-border bg-card p-4 shadow-sm">
  <!-- Header: Breadcrumbs Navigation -->
  <div class="flex items-center justify-between text-xs pb-2 border-b border-border">
    <div class="flex items-center gap-1.5 flex-wrap overflow-x-auto text-secondary">
      {#each breadcrumbStack as node, idx}
        <button
          type="button"
          onclick={() => handleBreadcrumbClick(idx)}
          class={`hover:text-accent font-medium transition-colors ${
            idx === breadcrumbStack.length - 1 ? 'text-primary font-bold' : ''
          }`}
        >
          {node.name || node.path}
        </button>
        {#if idx < breadcrumbStack.length - 1}
          <ChevronRight class="h-3 w-3 shrink-0 text-disabled" />
        {/if}
      {/each}
    </div>

    {#if onRefreshRequested}
      <button
        type="button"
        onclick={onRefreshRequested}
        disabled={isLoading}
        class="text-accent hover:underline text-xs font-medium disabled:opacity-50"
      >
        {t('Làm mới')}
      </button>
    {/if}
  </div>

  <!-- Treemap Canvas Area -->
  <div
    class="relative w-full h-[420px] rounded-lg overflow-hidden bg-subtle select-none border border-border/40"
    bind:clientWidth={containerWidth}
    bind:clientHeight={containerHeight}
  >
    {#if isLoading}
      <div class="absolute inset-0 z-20 flex flex-col items-center justify-center bg-card/70 backdrop-blur-sm gap-2">
        <div class="h-8 w-8 animate-spin rounded-full border-2 border-accent border-t-transparent"></div>
        <span class="text-xs text-secondary">{t('Đang phân tích không gian đĩa...')}</span>
      </div>
    {/if}

    {#if layoutRects.length === 0 && !isLoading}
      <div class="absolute inset-0 flex items-center justify-center text-xs text-secondary">
        {t('Chưa có dữ liệu Space Lens. Hãy bấm quét để xem bản đồ phân bổ.')}
      </div>
    {:else}
      {#each layoutRects as rect}
        <div
          role="button"
          tabindex="0"
          class="absolute overflow-hidden p-1.5 border border-black/10 transition-transform duration-75 cursor-pointer hover:brightness-110 hover:z-10"
          style="left: {rect.x}px; top: {rect.y}px; width: {rect.w}px; height: {rect.h}px; background-color: {rect.color};"
          onclick={() => handleDrillDown(rect.node)}
          onkeydown={(e) => {
            if (e.key === 'Enter' || e.key === ' ') {
              e.preventDefault();
              handleDrillDown(rect.node);
            }
          }}
          oncontextmenu={(e) => handleContextMenu(e, rect.node)}
          onmouseenter={(e) => {
            tooltip = {
              x: e.clientX,
              y: e.clientY,
              visible: true,
              node: rect.node
            };
          }}
          onmousemove={(e) => {
            tooltip.x = e.clientX;
            tooltip.y = e.clientY;
          }}
          onmouseleave={() => (tooltip.visible = false)}
        >
          {#if rect.w > 48 && rect.h > 32}
            <div class="flex flex-col text-white drop-shadow leading-tight overflow-hidden pointer-events-none">
              <span class="font-bold text-xs truncate">{rect.node.name}</span>
              <span class="text-[10px] opacity-90 truncate">{formatSize(rect.node.physical_bytes)}</span>
            </div>
          {/if}
        </div>
      {/each}
    {/if}
  </div>

  <!-- Hover Tooltip -->
  {#if tooltip.visible && tooltip.node}
    {@const node = tooltip.node}
    {@const pct = currentNode && currentNode.physical_bytes > 0 ? (node.physical_bytes / currentNode.physical_bytes) * 100 : 0}
    <div
      class="fixed z-40 pointer-events-none rounded-lg bg-card border border-border p-2.5 shadow-xl text-xs space-y-1"
      style="left: {tooltip.x + 12}px; top: {tooltip.y + 12}px;"
    >
      <div class="font-bold text-primary flex items-center gap-1.5">
        {#if node.is_dir}
          <Folder class="h-3.5 w-3.5 text-accent" />
        {:else}
          <File class="h-3.5 w-3.5 text-secondary" />
        {/if}
        <span>{node.name}</span>
      </div>
      <div class="text-[11px] text-secondary truncate max-w-xs">{node.path}</div>
      <div class="flex items-center gap-3 pt-1 text-primary tabular-numbers">
        <span>{formatSize(node.physical_bytes)}</span>
        <span>·</span>
        <span>{node.file_count} {t('tệp')}</span>
        <span>·</span>
        <span class="text-accent font-semibold">{pct.toFixed(1)}%</span>
      </div>
    </div>
  {/if}

  <!-- Right-Click Context Menu -->
  {#if contextMenu.visible && contextMenu.node}
    {@const node = contextMenu.node}
    <div
      class="fixed z-50 rounded-lg bg-card border border-border shadow-2xl py-1 text-xs w-48 animate-in fade-in zoom-in-95"
      style="left: {contextMenu.x}px; top: {contextMenu.y}px;"
    >
      <button
        type="button"
        onclick={() => revealInExplorer(node.path)}
        class="w-full text-start px-3 py-2 flex items-center gap-2 text-primary hover:bg-subtle transition-colors"
      >
        <ExternalLink class="h-3.5 w-3.5 text-secondary" />
        {t('Mở trong Explorer')}
      </button>

      <button
        type="button"
        onclick={() => copyPath(node.path)}
        class="w-full text-start px-3 py-2 flex items-center gap-2 text-primary hover:bg-subtle transition-colors"
      >
        <Copy class="h-3.5 w-3.5 text-secondary" />
        {t('Sao chép đường dẫn')}
      </button>
    </div>
  {/if}

  <!-- Toast Notification for Copy -->
  {#if copiedToast}
    <div class="fixed bottom-6 right-6 z-50 bg-primary text-card px-3.5 py-2 rounded-lg text-xs font-medium shadow-lg flex items-center gap-2 animate-in fade-in slide-in-from-bottom-2">
      <Check class="h-3.5 w-3.5 text-accent" />
      {t('Đã sao chép đường dẫn')}
    </div>
  {/if}
</div>
