<!-- ui/src/pages/VaultPage.svelte -->
<script lang="ts">
  import { onMount } from 'svelte';
  import { t, formatSize, i18n } from '$lib/i18n.svelte';
  import {
    getVaultArchives,
    restoreVaultArchive,
    deleteVaultArchive,
    inspectVaultArchive,
    restoreVaultFilesSelective,
    type VaultArchiveDto,
    type VaultFileItemDto,
    type VaultArchiveDetailDto,
    type VaultRestoreReportDto,
  } from '$lib/api';
  import {
    Archive,
    RotateCcw,
    Trash2,
    Search,
    Eye,
    ShieldCheck,
    AlertCircle,
    CheckCircle,
    Clock,
    X,
    FileText,
    RefreshCw,
    HardDrive,
    Lock,
    CheckSquare,
    Square
  } from 'lucide-svelte';

  // Runes State
  let archives = $state<VaultArchiveDto[]>([]);
  let isLoading = $state(false);
  let statusFilter = $state<'ALL' | 'QUARANTINED' | 'ROLLED_BACK' | 'PURGED'>('ALL');

  // Inspection Modal State
  let inspectingDetail = $state<VaultArchiveDetailDto | null>(null);
  let showInspectModal = $state(false);
  let searchQuery = $state('');
  let selectedFileArcnames = $state<string[]>([]);
  let isRestoring = $state(false);
  let restoreReport = $state<VaultRestoreReportDto | null>(null);

  // Filtered archives based on status
  let filteredArchives = $derived(
    archives.filter((a) => {
      if (statusFilter === 'ALL') return true;
      return a.status.toUpperCase() === statusFilter;
    })
  );

  // Total statistics across archives
  let totalArchivesCount = $derived(archives.length);
  let quarantinedArchives = $derived(
    archives.filter((a) => a.status.toUpperCase() === 'QUARANTINED')
  );
  let totalProtectedBytes = $derived(
    quarantinedArchives.reduce((acc, a) => acc + a.original_bytes, 0)
  );
  let totalCompressedVaultBytes = $derived(
    quarantinedArchives.reduce((acc, a) => acc + a.compressed_bytes, 0)
  );
  let spaceSavedBytes = $derived(
    Math.max(0, totalProtectedBytes - totalCompressedVaultBytes)
  );

  // Filtered files in inspect modal
  let filteredFiles = $derived(
    inspectingDetail
      ? inspectingDetail.files.filter((f) =>
          f.original_path.toLowerCase().includes(searchQuery.toLowerCase()) ||
          f.rel_arcname.toLowerCase().includes(searchQuery.toLowerCase())
        )
      : []
  );

  async function loadArchives() {
    isLoading = true;
    try {
      archives = await getVaultArchives();
    } catch (err) {
      console.error(t('Lỗi tải danh sách kho lưu trữ:'), err);
    } finally {
      isLoading = false;
    }
  }

  async function handleFullRestore(archive: VaultArchiveDto) {
    const confirmRestore = confirm(
      t('Bạn có chắc chắn muốn khôi phục toàn bộ {count} tệp tin từ kho "{name}"?', {
        count: archive.file_count,
        name: archive.app_name,
      })
    );
    if (!confirmRestore) return;

    isRestoring = true;
    try {
      const res = await restoreVaultArchive(archive.archive_id);
      restoreReport = res;
      alert(t('Đã khôi phục {count} tệp về vị trí cũ.', { count: res.restored_count }));
      await loadArchives();
    } catch (err) {
      alert(`${t('Lỗi khôi phục:')} ${err}`);
    } finally {
      isRestoring = false;
    }
  }

  async function handlePurgeArchive(archive: VaultArchiveDto) {
    const confirmPurge = confirm(
      t('Hành động này sẽ XÓA VĨNH VIỄN kho lưu trữ "{name}" khỏi ổ đĩa. Bạn không thể hoàn tác sau khi xóa. Bạn có chắc chắn?', {
        name: archive.app_name,
      })
    );
    if (!confirmPurge) return;

    try {
      await deleteVaultArchive(archive.archive_id);
      await loadArchives();
    } catch (err) {
      alert(`${t('Lỗi xóa kho lưu trữ:')} ${err}`);
    }
  }

  async function handleInspect(archive: VaultArchiveDto) {
    try {
      inspectingDetail = await inspectVaultArchive(archive.archive_id);
      searchQuery = '';
      selectedFileArcnames = [];
      showInspectModal = true;
    } catch (err) {
      alert(`${t('Không thể đọc chi tiết kho:')} ${err}`);
    }
  }

  function toggleFileSelection(arcname: string) {
    if (selectedFileArcnames.includes(arcname)) {
      selectedFileArcnames = selectedFileArcnames.filter((name) => name !== arcname);
    } else {
      selectedFileArcnames = [...selectedFileArcnames, arcname];
    }
  }

  function toggleSelectAllFiles() {
    if (selectedFileArcnames.length === filteredFiles.length) {
      selectedFileArcnames = [];
    } else {
      selectedFileArcnames = filteredFiles.map((f) => f.rel_arcname);
    }
  }

  async function handleSelectiveRestore() {
    if (!inspectingDetail || selectedFileArcnames.length === 0) return;

    isRestoring = true;
    try {
      const res = await restoreVaultFilesSelective(
        inspectingDetail.archive.archive_id,
        selectedFileArcnames
      );
      alert(t('Đã khôi phục chọn lọc {count} tệp tin.', { count: res.restored_count }));
      // Refresh inspect details
      inspectingDetail = await inspectVaultArchive(inspectingDetail.archive.archive_id);
      selectedFileArcnames = [];
    } catch (err) {
      alert(`${t('Lỗi khôi phục chọn lọc:')} ${err}`);
    } finally {
      isRestoring = false;
    }
  }

  onMount(() => {
    loadArchives();
  });
</script>

<div class="space-y-6">
  <!-- Stats Header Cards -->
  <div class="grid grid-cols-1 md:grid-cols-4 gap-4">
    <div class="rounded-xl border border-border bg-card p-4 shadow-sm">
      <div class="flex items-center justify-between text-secondary">
        <span class="text-xs uppercase font-medium">{t('Kho đang cách ly')}</span>
        <Archive class="h-4 w-4 text-accent" />
      </div>
      <div class="mt-2 text-2xl font-bold text-primary">
        {quarantinedArchives.length}
        <span class="text-xs font-normal text-secondary">/ {t('{total} tổng', { total: totalArchivesCount })}</span>
      </div>
    </div>

    <div class="rounded-xl border border-border bg-card p-4 shadow-sm">
      <div class="flex items-center justify-between text-secondary">
        <span class="text-xs uppercase font-medium">{t('Dữ liệu được bảo vệ')}</span>
        <ShieldCheck class="h-4 w-4 text-emerald-600" />
      </div>
      <div class="mt-2 text-2xl font-bold text-emerald-600">
        {formatSize(totalProtectedBytes)}
      </div>
    </div>

    <div class="rounded-xl border border-border bg-card p-4 shadow-sm">
      <div class="flex items-center justify-between text-secondary">
        <span class="text-xs uppercase font-medium">{t('Dung lượng nén trong kho')}</span>
        <HardDrive class="h-4 w-4 text-blue-500" />
      </div>
      <div class="mt-2 text-2xl font-bold text-primary">
        {formatSize(totalCompressedVaultBytes)}
      </div>
    </div>

    <div class="rounded-xl border border-border bg-card p-4 shadow-sm">
      <div class="flex items-center justify-between text-secondary">
        <span class="text-xs uppercase font-medium">{t('Tiết kiệm thực tế')}</span>
        <CheckCircle class="h-4 w-4 text-emerald-500" />
      </div>
      <div class="mt-2 text-2xl font-bold text-emerald-600">
        +{formatSize(spaceSavedBytes)}
      </div>
    </div>
  </div>

  <!-- Filters & Controls Bar -->
  <div class="flex flex-wrap items-center justify-between gap-4">
    <!-- Status Filter Buttons -->
    <div class="flex items-center gap-1.5 bg-subtle p-1 rounded-lg border border-border">
      <button
        class={`px-3 py-1 text-xs font-medium rounded-md transition-colors ${
          statusFilter === 'ALL' ? 'bg-card text-primary shadow-sm' : 'text-secondary hover:text-primary'
        }`}
        onclick={() => statusFilter = 'ALL'}
      >
        {t('Tất cả')} ({archives.length})
      </button>
      <button
        class={`px-3 py-1 text-xs font-medium rounded-md transition-colors ${
          statusFilter === 'QUARANTINED' ? 'bg-card text-emerald-600 shadow-sm' : 'text-secondary hover:text-primary'
        }`}
        onclick={() => statusFilter = 'QUARANTINED'}
      >
        {t('Đang cách ly')} ({quarantinedArchives.length})
      </button>
      <button
        class={`px-3 py-1 text-xs font-medium rounded-md transition-colors ${
          statusFilter === 'ROLLED_BACK' ? 'bg-card text-blue-600 shadow-sm' : 'text-secondary hover:text-primary'
        }`}
        onclick={() => statusFilter = 'ROLLED_BACK'}
      >
        {t('Đã hoàn tác')}
      </button>
      <button
        class={`px-3 py-1 text-xs font-medium rounded-md transition-colors ${
          statusFilter === 'PURGED' ? 'bg-card text-secondary shadow-sm' : 'text-secondary hover:text-primary'
        }`}
        onclick={() => statusFilter = 'PURGED'}
      >
        {t('Đã dọn sạch')}
      </button>
    </div>

    <button
      class="inline-flex items-center gap-1.5 px-3 py-1.5 rounded-md border border-border bg-card text-xs font-medium text-secondary hover:bg-subtle transition-colors"
      disabled={isLoading}
      onclick={loadArchives}
    >
      <RefreshCw class={`h-3.5 w-3.5 ${isLoading ? 'animate-spin' : ''}`} />
      {t('Làm mới')}
    </button>
  </div>

  <!-- Archives Table -->
  <div class="rounded-xl border border-border bg-card shadow-sm overflow-hidden">
    {#if isLoading}
      <div class="p-12 text-center text-secondary">
        <RefreshCw class="h-8 w-8 animate-spin mx-auto text-accent mb-3" />
        <p class="text-sm">{t('Đang tải danh sách kho lưu trữ...')}</p>
      </div>
    {:else if filteredArchives.length === 0}
      <div class="p-12 text-center text-secondary">
        <Archive class="h-8 w-8 text-disabled mx-auto mb-2" />
        <p class="text-sm font-medium text-primary">{t('Không có kho lưu trữ nào')}</p>
        <p class="text-xs mt-1">{t('Khi bạn dọn dẹp với chế độ Canary Vault bật, các kho lưu trữ sẽ xuất hiện tại đây.')}</p>
      </div>
    {:else}
      <div class="overflow-x-auto">
        <table class="w-full text-left text-xs border-collapse">
          <thead>
            <tr class="border-b border-border bg-subtle/50 text-secondary font-semibold uppercase tracking-wider">
              <th class="py-3 px-4">{t('Ứng dụng / Tên kho')}</th>
              <th class="py-3 px-4">{t('Thời gian tạo')}</th>
              <th class="py-3 px-4">{t('Thời hạn cách ly')}</th>
              <th class="py-3 px-4 text-right">{t('Dung lượng gốc')}</th>
              <th class="py-3 px-4 text-right">{t('Dung lượng nén')}</th>
              <th class="py-3 px-4 text-center">{t('Trạng thái')}</th>
              <th class="py-3 px-4 text-right">{t('Thao tác')}</th>
            </tr>
          </thead>
          <tbody class="divide-y divide-border/60">
            {#each filteredArchives as arch}
              <tr class="hover:bg-subtle/40 transition-colors">
                <td class="py-3 px-4">
                  <div class="font-semibold text-primary">{arch.app_name}</div>
                  <div class="text-[11px] text-disabled truncate max-w-xs" title={arch.zip_path}>
                    {arch.archive_id} • {t('{count} tệp', { count: arch.file_count })}
                  </div>
                </td>
                <td class="py-3 px-4 text-secondary">
                  {new Date(arch.created_at).toLocaleString(i18n.locale === 'vi' ? 'vi-VN' : undefined)}
                </td>
                <td class="py-3 px-4">
                  {#if arch.status.toUpperCase() === 'QUARANTINED'}
                    <span class="inline-flex items-center gap-1 font-medium text-emerald-600">
                      <Clock class="h-3.5 w-3.5" />
                      {t('Còn {days} ngày', { days: arch.remaining_days })}
                    </span>
                  {:else}
                    <span class="text-disabled">{t('Đã hết hạn')}</span>
                  {/if}
                </td>
                <td class="py-3 px-4 text-right font-medium text-primary">
                  {formatSize(arch.original_bytes)}
                </td>
                <td class="py-3 px-4 text-right font-medium text-secondary">
                  {formatSize(arch.compressed_bytes)}
                </td>
                <td class="py-3 px-4 text-center">
                  {#if arch.status.toUpperCase() === 'QUARANTINED'}
                    <span class="px-2 py-0.5 rounded-full text-[10px] font-semibold bg-emerald-500/10 text-emerald-600 border border-emerald-500/20">
                      {t('CÁCH LY')}
                    </span>
                  {:else if arch.status.toUpperCase() === 'ROLLED_BACK'}
                    <span class="px-2 py-0.5 rounded-full text-[10px] font-semibold bg-blue-500/10 text-blue-600 border border-blue-500/20">
                      {t('ĐÃ HOÀN TÁC')}
                    </span>
                  {:else if arch.status.toUpperCase() === 'MISSING'}
                    <span
                      class="px-2 py-0.5 rounded-full text-[10px] font-semibold bg-danger/10 text-danger border border-danger/20"
                      title={t('Tệp zip của gói này không còn trên đĩa nên không thể khôi phục.')}
                    >
                      {t('MẤT TỆP ZIP')}
                    </span>
                  {:else}
                    <span class="px-2 py-0.5 rounded-full text-[10px] font-semibold bg-gray-500/10 text-gray-500 border border-gray-500/20">
                      {t('ĐÃ DỌN SẠCH')}
                    </span>
                  {/if}
                </td>
                <td class="py-3 px-4 text-right">
                  <div class="flex items-center justify-end gap-1.5">
                    <button
                      class="p-1.5 rounded hover:bg-subtle text-secondary hover:text-primary transition-colors"
                      onclick={() => handleInspect(arch)}
                      title={t('Xem chi tiết danh sách tệp')}
                    >
                      <Eye class="h-4 w-4" />
                    </button>
                    {#if arch.status.toUpperCase() === 'QUARANTINED'}
                      <button
                        class="p-1.5 rounded hover:bg-emerald-500/10 text-emerald-600 transition-colors"
                        disabled={isRestoring}
                        onclick={() => handleFullRestore(arch)}
                        title={t('Hoàn tác khôi phục toàn bộ')}
                      >
                        <RotateCcw class="h-4 w-4" />
                      </button>
                      <button
                        class="p-1.5 rounded hover:bg-red-500/10 text-red-600 transition-colors"
                        onclick={() => handlePurgeArchive(arch)}
                        title={t('Xóa vĩnh viễn kho này')}
                      >
                        <Trash2 class="h-4 w-4" />
                      </button>
                    {/if}
                  </div>
                </td>
              </tr>
            {/each}
          </tbody>
        </table>
      </div>
    {/if}
  </div>
</div>

<!-- Drill-down File Inspection Modal -->
{#if showInspectModal && inspectingDetail}
  <div class="fixed inset-0 z-50 bg-black/50 backdrop-blur-sm flex items-center justify-center p-4">
    <div class="bg-card border border-border rounded-2xl max-w-4xl w-full h-[85vh] shadow-2xl flex flex-col overflow-hidden">
      <!-- Modal Header -->
      <div class="px-6 py-4 border-b border-border flex items-center justify-between">
        <div>
          <div class="flex items-center gap-2">
            <Archive class="h-5 w-5 text-accent" />
            <h3 class="font-bold text-base text-primary">
              {inspectingDetail.archive.app_name}
            </h3>
            <span class="text-xs text-secondary">
              ({t('{count} tệp', { count: inspectingDetail.total_files })} • {formatSize(inspectingDetail.archive.original_bytes)})
            </span>
          </div>
          <p class="text-xs text-disabled mt-0.5">{inspectingDetail.archive.zip_path}</p>
        </div>
        <button
          class="text-secondary hover:text-primary p-1 rounded-md"
          onclick={() => showInspectModal = false}
        >
          <X class="h-5 w-5" />
        </button>
      </div>

      <!-- Search & Filter Controls -->
      <div class="px-6 py-3 border-b border-border/60 bg-subtle/50 flex items-center justify-between gap-4">
        <div class="relative flex-1 max-w-md">
          <Search class="h-3.5 w-3.5 absolute left-3 top-1/2 -translate-y-1/2 text-secondary" />
          <input
            type="text"
            placeholder={t('Tìm kiếm tệp theo đường dẫn...')}
            class="w-full pl-9 pr-3 py-1.5 rounded-lg border border-border bg-card text-xs text-primary focus:outline-none focus:ring-1 focus:ring-accent"
            bind:value={searchQuery}
          />
        </div>

        {#if inspectingDetail.archive.status.toUpperCase() === 'QUARANTINED'}
          <div class="flex items-center gap-2">
            <span class="text-xs text-secondary">
              {t('Đã chọn {count} tệp', { count: selectedFileArcnames.length })}
            </span>
            <button
              class="px-3 py-1.5 rounded-lg bg-emerald-600 text-white text-xs font-medium hover:bg-emerald-700 disabled:opacity-50"
              disabled={selectedFileArcnames.length === 0 || isRestoring}
              onclick={handleSelectiveRestore}
            >
              {t('Khôi phục tệp đã chọn')}
            </button>
          </div>
        {/if}
      </div>

      <!-- File List Table -->
      <div class="flex-1 overflow-auto p-6">
        <table class="w-full text-left text-xs border-collapse">
          <thead>
            <tr class="border-b border-border bg-subtle text-secondary font-semibold uppercase tracking-wider">
              <th class="py-2.5 px-3 w-8">
                <button onclick={toggleSelectAllFiles} title={t('Chọn tất cả')}>
                  {#if selectedFileArcnames.length === filteredFiles.length && filteredFiles.length > 0}
                    <CheckSquare class="h-3.5 w-3.5 text-accent" />
                  {:else}
                    <Square class="h-3.5 w-3.5" />
                  {/if}
                </button>
              </th>
              <th class="py-2.5 px-3">{t('Đường dẫn gốc')}</th>
              <th class="py-2.5 px-3 text-right">{t('Kích thước')}</th>
              <th class="py-2.5 px-3">{t('Mã SHA-256')}</th>
              <th class="py-2.5 px-3 text-center">{t('HardLink')}</th>
              <th class="py-2.5 px-3 text-center">{t('Trạng thái')}</th>
            </tr>
          </thead>
          <tbody class="divide-y divide-border/60">
            {#each filteredFiles as f}
              <tr class="hover:bg-subtle/30 transition-colors">
                <td class="py-2 px-3">
                  <button onclick={() => toggleFileSelection(f.rel_arcname)}>
                    {#if selectedFileArcnames.includes(f.rel_arcname)}
                      <CheckSquare class="h-3.5 w-3.5 text-accent" />
                    {:else}
                      <Square class="h-3.5 w-3.5 text-secondary" />
                    {/if}
                  </button>
                </td>
                <td class="py-2 px-3 font-mono text-[11px] text-primary truncate max-w-md" title={f.original_path}>
                  {f.original_path}
                </td>
                <td class="py-2 px-3 text-right font-medium text-primary">
                  {formatSize(f.size_bytes)}
                </td>
                <td class="py-2 px-3 font-mono text-[10px] text-disabled truncate max-w-[120px]" title={f.sha256}>
                  {f.sha256}
                </td>
                <td class="py-2 px-3 text-center text-secondary">
                  {f.hardlink_count > 1 ? t('{count} liên kết', { count: f.hardlink_count }) : '1'}
                </td>
                <td class="py-2 px-3 text-center">
                  {#if f.restored_individually}
                    <span class="text-blue-600 font-medium text-[10px]">{t('Đã khôi phục')}</span>
                  {:else}
                    <span class="text-emerald-600 font-medium text-[10px]">{t('Trong kho')}</span>
                  {/if}
                </td>
              </tr>
            {/each}
          </tbody>
        </table>
      </div>

      <!-- Modal Footer -->
      <div class="px-6 py-3 border-t border-border bg-subtle/50 flex justify-end">
        <button
          class="px-4 py-1.5 rounded-lg border border-border bg-card text-xs font-medium text-secondary hover:bg-subtle"
          onclick={() => showInspectModal = false}
        >
          {t('Đóng')}
        </button>
      </div>
    </div>
  </div>
{/if}
