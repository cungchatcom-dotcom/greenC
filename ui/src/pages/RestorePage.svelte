<!-- ui/src/pages/RestorePage.svelte: Unified plain-language 1-click Restore filtered by Date or App -->
<script lang="ts">
  import { onMount } from 'svelte';
  import { t, formatSize, i18n } from '$lib/i18n.svelte';
  import {
    getVaultArchives,
    restoreVaultArchive,
    inspectVaultArchive,
    restoreVaultFilesSelective,
    listRelocationHistory,
    undoRelocation,
    getZaloStatus,
    restoreZaloRetainedOriginal,
    type VaultArchiveDto,
    type VaultArchiveDetailDto,
    type RelocationResultDto,
    type ZaloSafetyCopyDto,
  } from '$lib/api';
  import { fileActions, onFilesChanged } from '$lib/busy.svelte';
  import { taskStore } from '$lib/tasks.svelte';
  import {
    RotateCcw,
    Calendar,
    Filter,
    RefreshCw,
    Archive,
    ArrowRightLeft,
    ShieldCheck,
    Eye,
    X,
    Search,
    CheckSquare,
    Square,
    CheckCircle2,
  } from 'lucide-svelte';

  type DatePreset = 'all' | 'today' | '7d' | '30d' | 'custom';

  interface RestoreEntry {
    uid: string;
    kind: 'cleaned' | 'moved' | 'safety_copy';
    appName: string;
    createdAt: string;
    dateKey: string; // YYYY-MM-DD
    sizeBytes: number;
    fileCount: number | null;
    canRestore: boolean;
    statusLabel: string;
    detail: string;
    archive?: VaultArchiveDto;
    relocation?: RelocationResultDto;
    safetyCopy?: ZaloSafetyCopyDto;
  }

  let entries = $state<RestoreEntry[]>([]);
  let isLoading = $state(false);
  let busyUid = $state<string | null>(null);
  let confirmUid = $state<string | null>(null);
  let busyDateKey = $state<string | null>(null);

  // Filters
  let datePreset = $state<DatePreset>('all');
  let customDate = $state('');
  let selectedApp = $state('ALL');
  let onlyRestorable = $state(true);

  // Simple file inspector modal (for Vault archives)
  let inspectingDetail = $state<VaultArchiveDetailDto | null>(null);
  let showInspectModal = $state(false);
  let fileSearch = $state('');
  let selectedArcnames = $state<string[]>([]);

  let canAct = $derived(fileActions.running === 0 && busyUid === null && busyDateKey === null);

  function toLocalDateKey(iso: string): string {
    const d = new Date(iso);
    if (Number.isNaN(d.getTime())) return iso.slice(0, 10) || '0000-00-00';
    const yr = d.getFullYear();
    const mo = String(d.getMonth() + 1).padStart(2, '0');
    const dy = String(d.getDate()).padStart(2, '0');
    return `${yr}-${mo}-${dy}`;
  }

  function formatDateHeading(dateKey: string): string {
    const todayKey = toLocalDateKey(new Date().toISOString());
    const yest = new Date();
    yest.setDate(yest.getDate() - 1);
    const yestKey = toLocalDateKey(yest.toISOString());

    const parts = dateKey.split('-');
    const pretty = parts.length === 3 ? `${parts[2]}/${parts[1]}/${parts[0]}` : dateKey;
    if (dateKey === todayKey) return `${t('Hôm nay')} · ${pretty}`;
    if (dateKey === yestKey) return `${t('Hôm qua')} · ${pretty}`;
    return pretty;
  }

  function formatTime(iso: string): string {
    const d = new Date(iso);
    if (Number.isNaN(d.getTime())) return '';
    return d.toLocaleTimeString(i18n.locale === 'vi' ? 'vi-VN' : undefined, {
      hour: '2-digit',
      minute: '2-digit',
    });
  }

  async function loadAll() {
    isLoading = true;
    try {
      const [archivesRes, relocRes, zaloRes] = await Promise.allSettled([
        getVaultArchives(),
        listRelocationHistory(),
        getZaloStatus(),
      ]);

      const merged: RestoreEntry[] = [];

      if (archivesRes.status === 'fulfilled') {
        for (const arch of archivesRes.value) {
          const st = arch.status.toUpperCase();
          const restorable = st === 'QUARANTINED';
          merged.push({
            uid: `vault-${arch.id}`,
            kind: 'cleaned',
            appName: arch.app_name === 'QuickCleanup' ? t('Dọn dẹp hệ thống') : arch.app_name,
            createdAt: arch.created_at,
            dateKey: toLocalDateKey(arch.created_at),
            sizeBytes: arch.original_bytes,
            fileCount: arch.file_count,
            canRestore: restorable,
            statusLabel: restorable
              ? t('Đang giữ dự phòng (còn {days} ngày)', { days: arch.remaining_days })
              : st === 'ROLLED_BACK'
                ? t('Đã khôi phục')
                : t('Đã hết hạn'),
            detail: t('{count} tệp đã nén an toàn', { count: arch.file_count }),
            archive: arch,
          });
        }
      }

      if (relocRes.status === 'fulfilled') {
        for (const rec of relocRes.value) {
          const restorable = rec.status === 'done' && rec.strategy !== 'guided' && rec.id != null;
          merged.push({
            uid: `reloc-${rec.id ?? rec.created_at}`,
            kind: 'moved',
            appName: rec.app_name,
            createdAt: rec.created_at,
            dateKey: toLocalDateKey(rec.created_at),
            sizeBytes: rec.size_bytes,
            fileCount: null,
            canRestore: restorable,
            statusLabel: restorable
              ? t('Đã chuyển sang ổ dự phòng')
              : rec.status === 'undone'
                ? t('Đã đưa về ổ C:')
                : rec.status,
            detail: rec.target_path,
            relocation: rec,
          });
        }
      }

      if (zaloRes.status === 'fulfilled' && zaloRes.value.safety_copies) {
        for (const copy of zaloRes.value.safety_copies) {
          if (copy.id != null && copy.kind === 'original' && copy.exists_on_disk) {
            const ts = copy.created_at ?? new Date().toISOString();
            merged.push({
              uid: `zalo-copy-${copy.id}`,
              kind: 'safety_copy',
              appName: 'Zalo PC',
              createdAt: ts,
              dateKey: toLocalDateKey(ts),
              sizeBytes: copy.size_bytes,
              fileCount: copy.file_count,
              canRestore: true,
              statusLabel: t('Bản gốc giữ tạm 24h'),
              detail: copy.path,
              safetyCopy: copy,
            });
          }
        }
      }

      merged.sort((a, b) => b.createdAt.localeCompare(a.createdAt));
      entries = merged;
    } finally {
      isLoading = false;
    }
  }

  onFilesChanged(() => loadAll());

  onMount(() => {
    loadAll();
  });

  let appNames = $derived(
    Array.from(new Set(entries.map((e) => e.appName))).sort((a, b) => a.localeCompare(b))
  );

  let filteredEntries = $derived.by(() => {
    const now = new Date();
    const todayKey = toLocalDateKey(now.toISOString());
    const cutoff7 = new Date(now);
    cutoff7.setDate(cutoff7.getDate() - 7);
    const cutoff30 = new Date(now);
    cutoff30.setDate(cutoff30.getDate() - 30);

    return entries.filter((e) => {
      if (onlyRestorable && !e.canRestore) return false;
      if (selectedApp !== 'ALL' && e.appName !== selectedApp) return false;
      if (datePreset === 'today' && e.dateKey !== todayKey) return false;
      if (datePreset === '7d' && new Date(e.createdAt) < cutoff7) return false;
      if (datePreset === '30d' && new Date(e.createdAt) < cutoff30) return false;
      if (datePreset === 'custom' && customDate && e.dateKey !== customDate) return false;
      return true;
    });
  });

  let groupedByDate = $derived.by(() => {
    const groups = new Map<string, RestoreEntry[]>();
    for (const item of filteredEntries) {
      const list = groups.get(item.dateKey) ?? [];
      list.push(item);
      groups.set(item.dateKey, list);
    }
    return Array.from(groups.entries()).map(([dateKey, items]) => ({
      dateKey,
      items,
      totalBytes: items.reduce((s, i) => s + i.sizeBytes, 0),
      restorableCount: items.filter((i) => i.canRestore).length,
    }));
  });

  let filteredInspectFiles = $derived(
    inspectingDetail
      ? inspectingDetail.files.filter((f) =>
          f.original_path.toLowerCase().includes(fileSearch.toLowerCase())
        )
      : []
  );

  async function executeRestoreEntry(entry: RestoreEntry) {
    busyUid = entry.uid;
    confirmUid = null;
    try {
      if (entry.kind === 'cleaned' && entry.archive) {
        const res = await restoreVaultArchive(entry.archive.archive_id);
        taskStore.showToast({
          kind: 'ok',
          title: t('Đã khôi phục {name}', { name: entry.appName }),
          detail: t('Đã đưa lại {count} tệp về vị trí cũ.', { count: res.restored_count }),
        });
      } else if (entry.kind === 'moved' && entry.relocation?.id != null) {
        await undoRelocation(entry.relocation.id);
        taskStore.showToast({
          kind: 'ok',
          title: t('Đã đưa {name} về ổ C:', { name: entry.appName }),
        });
      } else if (entry.kind === 'safety_copy' && entry.safetyCopy?.id != null) {
        const msg = await restoreZaloRetainedOriginal(entry.safetyCopy.id);
        taskStore.showToast({
          kind: 'ok',
          title: t('Đã khôi phục bản gốc Zalo PC'),
          detail: msg,
        });
      }
    } catch (err) {
      taskStore.showToast({
        kind: 'err',
        title: t('Không khôi phục được {name}', { name: entry.appName }),
        detail: String(err),
      });
    } finally {
      busyUid = null;
    }
  }

  async function restoreWholeDate(dateKey: string, items: RestoreEntry[]) {
    const targets = items.filter((i) => i.canRestore);
    if (targets.length === 0) return;
    busyDateKey = dateKey;
    let ok = 0;
    let fail = 0;
    for (const item of targets) {
      try {
        if (item.kind === 'cleaned' && item.archive) {
          await restoreVaultArchive(item.archive.archive_id);
        } else if (item.kind === 'moved' && item.relocation?.id != null) {
          await undoRelocation(item.relocation.id);
        } else if (item.kind === 'safety_copy' && item.safetyCopy?.id != null) {
          await restoreZaloRetainedOriginal(item.safetyCopy.id);
        }
        ok++;
      } catch {
        fail++;
      }
    }
    busyDateKey = null;
    taskStore.showToast({
      kind: fail === 0 ? 'ok' : 'info',
      title: t('Hoàn tất khôi phục ngày {date}', { date: formatDateHeading(dateKey) }),
      detail:
        fail === 0
          ? t('Đã khôi phục thành công {count} mục.', { count: ok })
          : t('Thành công {ok} mục · Lỗi {fail} mục.', { ok, fail }),
    });
  }

  async function openInspect(entry: RestoreEntry) {
    if (!entry.archive) return;
    try {
      inspectingDetail = await inspectVaultArchive(entry.archive.archive_id);
      fileSearch = '';
      selectedArcnames = [];
      showInspectModal = true;
    } catch (err) {
      taskStore.showToast({
        kind: 'err',
        title: t('Không đọc được danh sách tệp'),
        detail: String(err),
      });
    }
  }

  function toggleInspectFile(arcname: string) {
    if (selectedArcnames.includes(arcname)) {
      selectedArcnames = selectedArcnames.filter((a) => a !== arcname);
    } else {
      selectedArcnames = [...selectedArcnames, arcname];
    }
  }

  async function restoreSelectedFiles() {
    if (!inspectingDetail || selectedArcnames.length === 0) return;
    try {
      const res = await restoreVaultFilesSelective(
        inspectingDetail.archive.archive_id,
        selectedArcnames
      );
      taskStore.showToast({
        kind: 'ok',
        title: t('Đã khôi phục {count} tệp tin', { count: res.restored_count }),
      });
      inspectingDetail = await inspectVaultArchive(inspectingDetail.archive.archive_id);
      selectedArcnames = [];
    } catch (err) {
      taskStore.showToast({
        kind: 'err',
        title: t('Lỗi khôi phục tệp'),
        detail: String(err),
      });
    }
  }
</script>

<div class="max-w-6xl mx-auto w-full space-y-5">
  <!-- Header -->
  <div class="flex flex-col sm:flex-row sm:items-center justify-between gap-3">
    <div>
      <h1 class="text-xl font-bold text-primary flex items-center gap-2">
        <RotateCcw class="h-5 w-5 text-accent" />
        {t('Khôi phục dữ liệu')}
      </h1>
      <p class="text-xs text-secondary mt-0.5">
        {t('Chọn ngày hoặc ứng dụng bạn muốn lấy lại, rồi bấm Khôi phục — mọi thứ sẽ trở về đúng chỗ cũ.')}
      </p>
    </div>

    <button
      type="button"
      onclick={loadAll}
      disabled={isLoading}
      class="inline-flex items-center gap-1.5 px-3 py-1.5 rounded-lg border border-border bg-card hover:bg-subtle text-xs font-medium text-primary self-start"
    >
      <RefreshCw class={`h-3.5 w-3.5 ${isLoading ? 'animate-spin' : ''}`} />
      {t('Làm mới')}
    </button>
  </div>

  <!-- Simple Filter Bar: Date + App -->
  <div class="rounded-2xl border border-border bg-card p-4 shadow-sm flex flex-wrap items-center gap-3 justify-between">
    <!-- Date Filter -->
    <div class="flex flex-wrap items-center gap-2">
      <span class="inline-flex items-center gap-1 text-xs font-semibold text-secondary">
        <Calendar class="h-3.5 w-3.5" />
        {t('Ngày:')}
      </span>
      <div class="inline-flex items-center gap-1 bg-subtle p-1 rounded-lg border border-border">
        {#each [
          { id: 'all', label: 'Tất cả' },
          { id: 'today', label: 'Hôm nay' },
          { id: '7d', label: '7 ngày' },
          { id: '30d', label: '30 ngày' },
        ] as opt}
          <button
            type="button"
            onclick={() => {
              datePreset = opt.id as DatePreset;
              customDate = '';
            }}
            class={`px-2.5 py-1 rounded-md text-xs font-medium transition-colors ${
              datePreset === opt.id
                ? 'bg-card text-accent shadow-sm'
                : 'text-secondary hover:text-primary'
            }`}
          >
            {t(opt.label)}
          </button>
        {/each}
      </div>

      <input
        type="date"
        bind:value={customDate}
        onchange={() => {
          if (customDate) datePreset = 'custom';
          else datePreset = 'all';
        }}
        class="px-2.5 py-1 rounded-lg border border-border bg-subtle text-xs text-primary outline-none focus:border-accent"
        aria-label={t('Chọn ngày cụ thể')}
      />
    </div>

    <!-- App Filter + Restorable Toggle -->
    <div class="flex flex-wrap items-center gap-3">
      <div class="flex items-center gap-1.5">
        <Filter class="h-3.5 w-3.5 text-secondary" />
        <select
          bind:value={selectedApp}
          class="px-2.5 py-1.5 rounded-lg border border-border bg-subtle text-xs font-semibold text-primary outline-none cursor-pointer"
        >
          <option value="ALL">{t('Tất cả ứng dụng')}</option>
          {#each appNames as app}
            <option value={app}>{app}</option>
          {/each}
        </select>
      </div>

      <label class="flex items-center gap-1.5 text-xs text-secondary cursor-pointer select-none">
        <input
          type="checkbox"
          bind:checked={onlyRestorable}
          class="h-3.5 w-3.5 rounded border-border text-accent"
        />
        <span>{t('Chỉ hiện mục khôi phục được')}</span>
      </label>
    </div>
  </div>

  <!-- Timeline Grouped by Date -->
  {#if isLoading && entries.length === 0}
    <div class="rounded-2xl border border-border bg-card p-12 text-center text-secondary">
      <RefreshCw class="h-7 w-7 animate-spin mx-auto text-accent mb-2" />
      <p class="text-sm">{t('Đang tải lịch sử khôi phục...')}</p>
    </div>
  {:else if groupedByDate.length === 0}
    <div class="rounded-2xl border border-border bg-card p-12 text-center space-y-2">
      <CheckCircle2 class="h-8 w-8 text-emerald-500 mx-auto" />
      <p class="text-sm font-semibold text-primary">{t('Không có mục nào cần khôi phục')}</p>
      <p class="text-xs text-secondary">
        {t('Khi GreenC dọn rác vào kho dự phòng hoặc chuyển thư mục sang ổ khác, bạn có thể lấy lại bất cứ lúc nào tại đây.')}
      </p>
    </div>
  {:else}
    <div class="space-y-4">
      {#each groupedByDate as group (group.dateKey)}
        <section class="rounded-2xl border border-border bg-card shadow-sm overflow-hidden">
          <!-- Date Group Header -->
          <div class="px-5 py-3 bg-subtle/60 border-b border-border flex items-center justify-between gap-3 flex-wrap">
            <div class="flex items-center gap-2 text-xs">
              <span class="font-bold text-primary">{formatDateHeading(group.dateKey)}</span>
              <span class="text-secondary">
                · {t('{count} mục', { count: group.items.length })} · {formatSize(group.totalBytes)}
              </span>
            </div>

            {#if group.restorableCount > 1}
              <button
                type="button"
                disabled={!canAct}
                onclick={() => restoreWholeDate(group.dateKey, group.items)}
                class="inline-flex items-center gap-1.5 px-3 py-1 rounded-lg border border-accent/50 text-accent hover:bg-accent/10 text-xs font-semibold disabled:opacity-50"
              >
                <RotateCcw class="h-3.5 w-3.5" />
                {busyDateKey === group.dateKey
                  ? t('Đang khôi phục...')
                  : t('Khôi phục tất cả ngày này')}
              </button>
            {/if}
          </div>

          <!-- Items List -->
          <div class="divide-y divide-border/60">
            {#each group.items as item (item.uid)}
              <div class="px-5 py-3.5 flex flex-col sm:flex-row sm:items-center justify-between gap-3 hover:bg-subtle/30 transition-colors">
                <div class="flex items-start gap-3 min-w-0">
                  <div
                    class={`h-9 w-9 rounded-xl flex items-center justify-center shrink-0 mt-0.5 ${
                      item.kind === 'moved'
                        ? 'bg-blue-500/10 text-blue-600 dark:text-blue-400'
                        : item.kind === 'safety_copy'
                          ? 'bg-amber-500/10 text-amber-600 dark:text-amber-400'
                          : 'bg-emerald-500/10 text-emerald-600 dark:text-emerald-400'
                    }`}
                  >
                    {#if item.kind === 'moved'}
                      <ArrowRightLeft class="h-4 w-4" />
                    {:else if item.kind === 'safety_copy'}
                      <ShieldCheck class="h-4 w-4" />
                    {:else}
                      <Archive class="h-4 w-4" />
                    {/if}
                  </div>

                  <div class="min-w-0 space-y-0.5">
                    <div class="flex items-center gap-2 flex-wrap">
                      <span class="font-bold text-sm text-primary">{item.appName}</span>
                      <span class="text-xs font-mono font-semibold text-primary tabular-numbers">
                        {formatSize(item.sizeBytes)}
                      </span>
                      <span class="text-[11px] text-secondary">{formatTime(item.createdAt)}</span>
                    </div>
                    <div class="text-xs text-secondary flex items-center gap-2 flex-wrap">
                      <span>{item.statusLabel}</span>
                      {#if item.detail}
                        <span class="text-disabled truncate max-w-md" title={item.detail}>
                          • {item.detail}
                        </span>
                      {/if}
                    </div>
                  </div>
                </div>

                <!-- 1-Click Restore Action -->
                <div class="flex items-center gap-2 shrink-0">
                  {#if item.archive && item.canRestore}
                    <button
                      type="button"
                      onclick={() => openInspect(item)}
                      class="inline-flex items-center gap-1 px-2.5 py-1.5 rounded-lg border border-border hover:bg-subtle text-xs font-medium text-secondary hover:text-primary"
                    >
                      <Eye class="h-3.5 w-3.5" />
                      {t('Xem tệp')}
                    </button>
                  {/if}

                  {#if item.canRestore}
                    {#if confirmUid === item.uid}
                      <button
                        type="button"
                        onclick={() => (confirmUid = null)}
                        class="px-2.5 py-1.5 rounded-lg border border-border text-xs text-secondary"
                      >
                        {t('Hủy')}
                      </button>
                      <button
                        type="button"
                        disabled={!canAct}
                        onclick={() => executeRestoreEntry(item)}
                        class="px-3.5 py-1.5 rounded-lg bg-emerald-600 hover:bg-emerald-700 text-white text-xs font-semibold"
                      >
                        {t('Xác nhận khôi phục')}
                      </button>
                    {:else}
                      <button
                        type="button"
                        disabled={!canAct}
                        onclick={() => (confirmUid = item.uid)}
                        class="inline-flex items-center gap-1.5 px-3.5 py-1.5 rounded-lg bg-accent hover:bg-accent-hover text-white text-xs font-semibold disabled:opacity-50"
                      >
                        <RotateCcw class="h-3.5 w-3.5" />
                        {busyUid === item.uid ? t('Đang khôi phục...') : t('Khôi phục')}
                      </button>
                    {/if}
                  {/if}
                </div>
              </div>
            {/each}
          </div>
        </section>
      {/each}
    </div>
  {/if}
</div>

<!-- Simplified File Picker Modal (no SHA-256 / HardLink clutter) -->
{#if showInspectModal && inspectingDetail}
  <div class="fixed inset-0 z-50 bg-black/45 backdrop-blur-sm flex items-center justify-center p-4">
    <div class="bg-card border border-border rounded-2xl max-w-2xl w-full max-h-[80vh] shadow-2xl flex flex-col overflow-hidden">
      <div class="px-5 py-4 border-b border-border flex items-center justify-between">
        <div>
          <h3 class="font-bold text-base text-primary">{inspectingDetail.archive.app_name}</h3>
          <p class="text-xs text-secondary">
            {t('{count} tệp', { count: inspectingDetail.total_files })} · {formatSize(inspectingDetail.archive.original_bytes)}
          </p>
        </div>
        <button
          type="button"
          onclick={() => (showInspectModal = false)}
          class="text-secondary hover:text-primary"
        >
          <X class="h-5 w-5" />
        </button>
      </div>

      <div class="px-5 py-3 border-b border-border/60 bg-subtle/50 flex items-center justify-between gap-3">
        <div class="relative flex-1">
          <Search class="h-3.5 w-3.5 absolute left-3 top-1/2 -translate-y-1/2 text-secondary" />
          <input
            type="text"
            bind:value={fileSearch}
            placeholder={t('Tìm tên tệp...')}
            class="w-full pl-9 pr-3 py-1.5 rounded-lg border border-border bg-card text-xs text-primary outline-none focus:border-accent"
          />
        </div>
        <button
          type="button"
          disabled={selectedArcnames.length === 0 || !canAct}
          onclick={restoreSelectedFiles}
          class="px-3.5 py-1.5 rounded-lg bg-accent hover:bg-accent-hover text-white text-xs font-semibold disabled:opacity-50 shrink-0"
        >
          {t('Khôi phục ({count})', { count: selectedArcnames.length })}
        </button>
      </div>

      <div class="flex-1 overflow-y-auto divide-y divide-border/50 px-5 py-2">
        {#each filteredInspectFiles as file}
          {@const checked = selectedArcnames.includes(file.rel_arcname)}
          <button
            type="button"
            onclick={() => toggleInspectFile(file.rel_arcname)}
            class="w-full py-2.5 flex items-center justify-between gap-3 text-left hover:bg-subtle/40 rounded-md px-2 text-xs"
          >
            <span class="flex items-center gap-2 min-w-0">
              {#if checked}
                <CheckSquare class="h-4 w-4 text-accent shrink-0" />
              {:else}
                <Square class="h-4 w-4 text-secondary shrink-0" />
              {/if}
              <span class="truncate text-primary font-mono text-[11px]" title={file.original_path}>
                {file.original_path}
              </span>
            </span>
            <span class="text-secondary font-mono shrink-0">{formatSize(file.size_bytes)}</span>
          </button>
        {/each}
      </div>
    </div>
  </div>
{/if}
