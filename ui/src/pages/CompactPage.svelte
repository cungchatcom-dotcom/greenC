<!-- ui/src/pages/CompactPage.svelte -->
<script lang="ts">
  import { onMount } from 'svelte';
  import { t, formatSize } from '$lib/i18n.svelte';
  import {
    scanCompactionCandidates,
    compactFiles,
    uncompactFiles,
    compactVhdx,
    listCompactionHistory,
    listenTaskProgress,
    controlTask,
    type CompactionCandidateDto,
    type CompactionResultDto,
    type CompactionHistoryDto,
    type TaskProgressEvent
  } from '$lib/api';
  import {
    Minimize2,
    HardDrive,
    Layers,
    CheckSquare,
    Square,
    RefreshCw,
    AlertTriangle,
    CheckCircle2,
    Undo2,
    Sliders,
    Sparkles,
    Cpu,
    Zap,
    Folder
  } from 'lucide-svelte';

  // Svelte 5 Runes State
  let activeTab = $state<'wof' | 'vhdx'>('wof');
  let candidates = $state<CompactionCandidateDto[]>([]);
  let historyRecords = $state<CompactionHistoryDto[]>([]);
  let selectedAlgorithm = $state<'XPRESS4K' | 'XPRESS8K' | 'XPRESS16K' | 'LZX'>('XPRESS8K');

  let isLoading = $state(false);
  let isCompacting = $state(false);
  let currentTaskId = $state<string | null>(null);
  let progressState = $state<TaskProgressEvent | null>(null);
  let lastResult = $state<CompactionResultDto | null>(null);

  // Derived Values
  let wofCandidates = $derived(candidates.filter((c) => c.kind === 'wof_directory'));
  let vhdxCandidates = $derived(candidates.filter((c) => c.kind === 'vhdx_trim'));
  let selectedWof = $derived(wofCandidates.filter((c) => c.selected));
  let isAllWofSelected = $derived(
    wofCandidates.length > 0 && wofCandidates.every((c) => c.selected)
  );

  let totalSelectedLogical = $derived(
    selectedWof.reduce((sum, c) => sum + c.logical_bytes, 0)
  );
  let totalSelectedReclaimable = $derived(
    selectedWof.reduce((sum, c) => sum + c.estimated_reclaimable_bytes, 0)
  );

  async function loadData() {
    isLoading = true;
    try {
      const [candList, histList] = await Promise.all([
        scanCompactionCandidates(),
        listCompactionHistory(),
      ]);
      candidates = candList;
      historyRecords = histList;
    } catch (err) {
      console.error('Failed to scan compaction data:', err);
    } finally {
      isLoading = false;
    }
  }

  function toggleSelectAllWof() {
    const nextState = !isAllWofSelected;
    for (const c of wofCandidates) {
      c.selected = nextState;
    }
  }

  async function handleCompactSelected() {
    const paths = selectedWof.map((c) => c.path);
    if (paths.length === 0) return;

    if (!confirm(t('Thu gọn {count} thư mục đã chọn với thuật toán {algo}?', {
      count: paths.length,
      algo: selectedAlgorithm
    }))) {
      return;
    }

    isCompacting = true;
    try {
      const res = await compactFiles(paths, selectedAlgorithm);
      lastResult = res;
      await loadData();
    } catch (err) {
      alert(t('Có lỗi') + ': ' + err);
    } finally {
      isCompacting = false;
      progressState = null;
      currentTaskId = null;
    }
  }

  async function handleUncompact(cand: CompactionCandidateDto) {
    if (!confirm(t('Giải nén và phục hồi dung lượng gốc cho {name}?', { name: cand.name }))) return;

    try {
      await uncompactFiles([cand.path]);
      await loadData();
    } catch (err) {
      alert(t('Có lỗi') + ': ' + err);
    }
  }

  async function handleCompactVhdxItem(cand: CompactionCandidateDto) {
    if (cand.is_locked) {
      alert(t('Cần đóng WSL2 / Docker trước khi thu gọn để tránh lỗi dữ liệu'));
      return;
    }

    if (!confirm(t('Thu gọn khoảng trống thừa của đĩa ảo {name}?', { name: cand.name }))) return;

    isCompacting = true;
    try {
      const res = await compactVhdx(cand.path);
      lastResult = res;
      await loadData();
    } catch (err) {
      alert(t('Có lỗi') + ': ' + err);
    } finally {
      isCompacting = false;
      progressState = null;
      currentTaskId = null;
    }
  }

  async function handleCancelTask() {
    if (currentTaskId) {
      await controlTask(currentTaskId, 'cancel');
    }
  }

  onMount(() => {
    loadData();

    const unlistenPromise = listenTaskProgress((event) => {
      if (event.kind === 'compact' || event.kind === 'compact_vhdx' || event.kind === 'compact_scan') {
        progressState = event;
        currentTaskId = event.task_id;
      }
    });

    return () => {
      unlistenPromise.then((unlisten) => unlisten());
    };
  });
</script>

<div class="flex flex-col gap-6 p-6 max-w-7xl mx-auto w-full">
  <!-- Header Bar -->
  <div class="flex flex-col sm:flex-row items-start sm:items-center justify-between gap-4">
    <div>
      <h1 class="text-2xl font-bold tracking-tight text-primary flex items-center gap-2">
        <Minimize2 class="h-6 w-6 text-accent" />
        {t('Thu gọn tại chỗ (Cold Compaction)')}
      </h1>
      <p class="text-sm text-secondary mt-1">
        {t('Nén trong suốt các thư mục SDK, toolchain và đĩa ảo WSL2 / Docker để giải phóng không gian mà không đổi đường dẫn.')}
      </p>
    </div>

    <div class="flex items-center gap-3">
      <button
        onclick={loadData}
        disabled={isLoading || isCompacting}
        class="flex items-center gap-2 bg-card border border-border hover:bg-subtle px-3 py-1.5 rounded-lg text-sm font-medium text-primary shadow-sm transition-colors"
      >
        <RefreshCw class={`h-4 w-4 ${isLoading ? 'animate-spin' : ''}`} />
        {t('Quét lại')}
      </button>
    </div>
  </div>

  <!-- Tab Buttons -->
  <div class="flex items-center gap-2 border-b border-border pb-3">
    <button
      onclick={() => (activeTab = 'wof')}
      class={`flex items-center gap-2 px-4 py-2 rounded-lg text-sm font-medium transition-colors ${
        activeTab === 'wof'
          ? 'bg-accent text-white shadow-sm'
          : 'bg-card border border-border text-secondary hover:text-primary'
      }`}
    >
      <Layers class="h-4 w-4" />
      {t('Thư mục & SDK')} ({wofCandidates.length})
    </button>

    <button
      onclick={() => (activeTab = 'vhdx')}
      class={`flex items-center gap-2 px-4 py-2 rounded-lg text-sm font-medium transition-colors ${
        activeTab === 'vhdx'
          ? 'bg-accent text-white shadow-sm'
          : 'bg-card border border-border text-secondary hover:text-primary'
      }`}
    >
      <HardDrive class="h-4 w-4" />
      {t('Đĩa ảo .vhdx (WSL / Docker)')} ({vhdxCandidates.length})
    </button>
  </div>

  {#if activeTab === 'wof'}
    <!-- WOF Compaction Tab -->
    <div class="rounded-xl border border-border bg-card p-5 shadow-sm space-y-5">
      <!-- Algorithm Selector & Summary Bar -->
      <div class="flex flex-col md:flex-row items-start md:items-center justify-between gap-4 p-4 rounded-xl bg-subtle/50 border border-border/80">
        <div class="flex flex-col gap-1">
          <span class="text-xs font-semibold text-secondary uppercase tracking-wider flex items-center gap-1.5">
            <Sliders class="h-3.5 w-3.5 text-accent" />
            {t('Thuật toán nén Windows CompactOS')}
          </span>
          <div class="flex items-center gap-2 mt-1">
            <select
              bind:value={selectedAlgorithm}
              class="bg-card border border-border rounded-lg px-3 py-1.5 text-sm font-semibold text-primary outline-none cursor-pointer"
            >
              <option value="XPRESS8K">XPRESS8K ({t('Mặc định, cân bằng tốc độ & dung lượng')})</option>
              <option value="XPRESS16K">XPRESS16K ({t('Nén cao hơn')})</option>
              <option value="XPRESS4K">XPRESS4K ({t('Nhanh nhất, tốn ít CPU')})</option>
              <option value="LZX">LZX ({t('Nén tối đa, đọc tốn CPU hơn')})</option>
            </select>
          </div>
        </div>

        <div class="flex items-center gap-4">
          <div class="text-right">
            <div class="text-xs text-secondary">{t('Đã chọn {count} thư mục', { count: selectedWof.length })}</div>
            <div class="text-sm font-bold text-emerald-600 dark:text-emerald-400">
              ~{formatSize(totalSelectedReclaimable)} {t('tiết kiệm')}
            </div>
          </div>

          <button
            onclick={handleCompactSelected}
            disabled={selectedWof.length === 0 || isCompacting}
            class="px-4 py-2 rounded-lg bg-accent text-white text-sm font-semibold hover:bg-accent-hover disabled:opacity-50 transition-colors shadow-sm flex items-center gap-2"
          >
            <Sparkles class="h-4 w-4" />
            {t('Thu gọn ngay')}
          </button>
        </div>
      </div>

      <!-- Candidate Folders Table -->
      {#if wofCandidates.length === 0}
        <div class="p-8 text-center text-sm text-secondary">
          {t('Không tìm thấy thư mục SDK hoặc tệp tĩnh lớn nào cần thu gọn.')}
        </div>
      {:else}
        <div class="overflow-x-auto">
          <table class="w-full text-left border-collapse text-sm">
            <thead>
              <tr class="border-b border-border text-xs text-secondary uppercase font-semibold">
                <th class="py-2.5 px-3 w-10">
                  <button onclick={toggleSelectAllWof} class="text-secondary hover:text-primary">
                    {#if isAllWofSelected}
                      <CheckSquare class="h-4 w-4 text-accent" />
                    {:else}
                      <Square class="h-4 w-4" />
                    {/if}
                  </button>
                </th>
                <th class="py-2.5 px-3">{t('Thư mục')}</th>
                <th class="py-2.5 px-3">{t('Đường dẫn')}</th>
                <th class="py-2.5 px-3 text-right">{t('Gốc → Nén')}</th>
                <th class="py-2.5 px-3 text-right">{t('Giải phóng')}</th>
                <th class="py-2.5 px-3 text-right">{t('Thao tác')}</th>
              </tr>
            </thead>
            <tbody class="divide-y divide-border/60">
              {#each wofCandidates as cand}
                <tr class="hover:bg-subtle/50 transition-colors">
                  <td class="py-3 px-3">
                    <input type="checkbox" bind:checked={cand.selected} class="rounded border-border cursor-pointer" />
                  </td>
                  <td class="py-3 px-3 font-semibold text-primary">
                    {cand.name}
                    {#if cand.is_compacted}
                      <span class="ml-2 text-xs font-normal text-emerald-600 dark:text-emerald-400">
                        ({t('Đã nén')})
                      </span>
                    {/if}
                  </td>
                  <td class="py-3 px-3 text-xs text-secondary font-mono truncate max-w-xs" title={cand.path}>
                    {cand.path}
                  </td>
                  <td class="py-3 px-3 text-right font-mono text-xs">
                    {formatSize(cand.logical_bytes)} → {formatSize(cand.physical_bytes)}
                  </td>
                  <td class="py-3 px-3 text-right font-mono text-xs text-emerald-600 dark:text-emerald-400 font-semibold">
                    -{formatSize(cand.estimated_reclaimable_bytes)}
                  </td>
                  <td class="py-3 px-3 text-right">
                    {#if cand.is_compacted}
                      <button
                        onclick={() => handleUncompact(cand)}
                        class="px-2.5 py-1 text-xs font-medium rounded border border-border hover:bg-subtle text-secondary hover:text-primary transition-colors"
                      >
                        <Undo2 class="h-3 w-3 inline mr-1" />
                        {t('Hoàn tác')}
                      </button>
                    {:else}
                      <span class="text-xs text-disabled">{t('Chưa nén')}</span>
                    {/if}
                  </td>
                </tr>
              {/each}
            </tbody>
          </table>
        </div>
      {/if}
    </div>
  {:else}
    <!-- VHDX Compaction Tab -->
    <div class="rounded-xl border border-border bg-card p-5 shadow-sm">
      <div class="flex items-center justify-between mb-4">
        <div>
          <h2 class="text-base font-semibold text-primary">{t('Ảnh đĩa ảo Container / Máy ảo (.vhdx / WSL / Docker)')}</h2>
          <p class="text-xs text-secondary mt-0.5">
            {t('Thu hồi khoảng trống rỗng bên trong đĩa ảo sau khi xóa file hoặc dọn container.')}
          </p>
        </div>
      </div>

      {#if vhdxCandidates.length === 0}
        <div class="p-8 text-center text-sm text-secondary">
          {t('Không tìm thấy đĩa ảo VHDX nào trên ổ C:.')}
        </div>
      {:else}
        <div class="space-y-4">
          {#each vhdxCandidates as vhdx}
            <div class="border border-border rounded-xl p-4 bg-subtle/30 flex flex-col sm:flex-row items-start sm:items-center justify-between gap-4">
              <div>
                <div class="flex items-center gap-2">
                  <HardDrive class="h-5 w-5 text-accent" />
                  <span class="font-bold text-primary text-base">{vhdx.name}</span>
                </div>
                <p class="text-xs text-secondary font-mono mt-1">{vhdx.path}</p>
                <div class="flex items-center gap-4 mt-2 text-xs font-mono">
                  <span class="text-secondary">{t('Dung lượng')}: {formatSize(vhdx.physical_bytes)}</span>
                  <span class="text-emerald-600 dark:text-emerald-400 font-medium">
                    {t('Có thể giải phóng')}: ~{formatSize(vhdx.estimated_reclaimable_bytes)}
                  </span>
                </div>
              </div>

              <div class="flex items-center gap-2">
                {#if vhdx.is_locked}
                  <span class="text-xs text-amber-600 dark:text-amber-400 flex items-center gap-1 font-medium mr-2">
                    <AlertTriangle class="h-3.5 w-3.5" />
                    {t('Cần đóng {app}', { app: vhdx.name })}
                  </span>
                {/if}
                <button
                  onclick={() => handleCompactVhdxItem(vhdx)}
                  disabled={isCompacting || vhdx.is_locked}
                  class="px-4 py-2 text-xs font-semibold rounded-lg bg-accent text-white hover:bg-accent-hover disabled:opacity-50 transition-colors shadow-sm"
                >
                  {t('Thu gọn')}
                </button>
              </div>
            </div>
          {/each}
        </div>
      {/if}
    </div>
  {/if}

  <!-- Section 3: Compaction History -->
  {#if historyRecords.length > 0}
    <div class="rounded-xl border border-border bg-card p-5 shadow-sm">
      <div class="flex items-center justify-between mb-4">
        <h2 class="text-base font-semibold text-primary">{t('Lịch sử thu gọn')}</h2>
        <span class="text-xs text-secondary">{historyRecords.length} {t('mục')}</span>
      </div>

      <div class="overflow-x-auto">
        <table class="w-full text-left border-collapse text-sm">
          <thead>
            <tr class="border-b border-border text-xs text-secondary uppercase font-semibold">
              <th class="py-2.5 px-3">{t('Đường dẫn')}</th>
              <th class="py-2.5 px-3">{t('Thuật toán')}</th>
              <th class="py-2.5 px-3 text-right">{t('Gốc → Thực tế')}</th>
              <th class="py-2.5 px-3 text-right">{t('Tiết kiệm')}</th>
              <th class="py-2.5 px-3 text-right">{t('Trạng thái')}</th>
            </tr>
          </thead>
          <tbody class="divide-y divide-border/60">
            {#each historyRecords as hist}
              {@const saved = Math.max(0, hist.physical_before_bytes - hist.physical_after_bytes)}
              <tr class="hover:bg-subtle/50 transition-colors">
                <td class="py-3 px-3 text-xs font-mono text-primary truncate max-w-sm" title={hist.target_path}>
                  {hist.target_path}
                </td>
                <td class="py-3 px-3 text-xs text-secondary font-mono">
                  {hist.algorithm}
                </td>
                <td class="py-3 px-3 text-right text-xs font-mono">
                  {formatSize(hist.physical_before_bytes)} → {formatSize(hist.physical_after_bytes)}
                </td>
                <td class="py-3 px-3 text-right text-xs font-mono text-emerald-600 dark:text-emerald-400 font-semibold">
                  -{formatSize(saved)}
                </td>
                <td class="py-3 px-3 text-right text-xs font-medium">
                  {#if hist.status === 'compacted'}
                    <span class="text-emerald-600 dark:text-emerald-400">{t('Đã nén')}</span>
                  {:else}
                    <span class="text-secondary">{t('Đã giải nén')}</span>
                  {/if}
                </td>
              </tr>
            {/each}
          </tbody>
        </table>
      </div>
    </div>
  {/if}
</div>

<!-- Compacting Progress Overlay -->
{#if isCompacting && progressState}
  <div class="fixed inset-0 z-50 bg-black/40 backdrop-blur-sm flex items-center justify-center p-4">
    <div class="rounded-xl border border-border bg-card p-6 shadow-2xl max-w-md w-full max-h-full overflow-y-auto">
      <div class="flex items-center justify-between mb-4">
        <h3 class="font-bold text-base text-primary flex items-center gap-2">
          <Minimize2 class="h-5 w-5 text-accent animate-pulse" />
          {progressState.title || t('Đang thu gọn tệp')}
        </h3>
        <button onclick={handleCancelTask} class="text-xs text-rose-500 hover:text-rose-600 font-medium">
          {t('Hủy')}
        </button>
      </div>

      <div class="w-full bg-subtle rounded-full h-2.5 mb-3 overflow-hidden">
        <div class="bg-accent h-2.5 rounded-full transition-all duration-150" style={`width: ${progressState.percent}%`}></div>
      </div>

      <div class="flex items-center justify-between text-xs text-secondary mb-2 font-mono">
        <span>{progressState.percent}%</span>
        <span>{progressState.done_files} {t('tệp')}</span>
      </div>

      <p class="text-xs text-secondary truncate font-mono bg-subtle p-2 rounded">
        {progressState.detail}
      </p>
    </div>
  </div>
{/if}
