<!-- ui/src/lib/components/GuidedModal.svelte -->
<script lang="ts">
  import { t, formatSize } from '$lib/i18n.svelte';
  import {
    revealInExplorer,
    listWslDistros,
    buildWslMoveCommand,
    launchWslMovePowershell,
    type RelocationCandidateDto,
    type WslDistroInfo,
    type WslMoveScript,
  } from '$lib/api';
  import { taskStore } from '$lib/tasks.svelte';
  import { X, RefreshCw, Folder, Terminal, Copy, Check, Play } from 'lucide-svelte';

  interface Props {
    open: boolean;
    candidate: RelocationCandidateDto | null;
    targetDrive: string;
    isStoreBuild?: boolean;
    onClose: () => void;
    onRecheck: () => void;
  }

  let {
    open,
    candidate,
    targetDrive,
    isStoreBuild = false,
    onClose,
    onRecheck,
  }: Props = $props();

  let wslDistros = $state<WslDistroInfo[]>([]);
  let selectedDistro = $state('Ubuntu');
  let wslScript = $state<WslMoveScript | null>(null);
  let loadingWsl = $state(false);
  let launchingWsl = $state(false);
  let copiedCmd = $state(false);
  let wslError = $state<string | null>(null);

  async function loadWslData() {
    loadingWsl = true;
    wslError = null;
    try {
      const list = await listWslDistros();
      wslDistros = list;
      const preferred = list.find((d) => !d.is_docker_internal) ?? list[0];
      if (preferred) {
        selectedDistro = preferred.name;
      }
      await refreshWslScript();
    } catch (err) {
      wslError = String(err);
    } finally {
      loadingWsl = false;
    }
  }

  async function refreshWslScript() {
    if (!selectedDistro.trim()) {
      wslScript = null;
      return;
    }
    try {
      wslScript = await buildWslMoveCommand(selectedDistro.trim(), targetDrive);
      wslError = null;
    } catch (err) {
      wslScript = null;
      wslError = String(err);
    }
  }

  $effect(() => {
    if (open && candidate?.id === 'wsl2') {
      void loadWslData();
    }
  });

  $effect(() => {
    void targetDrive;
    void selectedDistro;
    if (open && candidate?.id === 'wsl2' && !loadingWsl) {
      void refreshWslScript();
    }
  });

  async function handleLaunchPowershell() {
    if (!selectedDistro.trim() || launchingWsl) return;
    launchingWsl = true;
    wslError = null;
    try {
      await launchWslMovePowershell(selectedDistro.trim(), targetDrive);
      taskStore.showToast({
        kind: 'ok',
        title: t('Đã mở PowerShell tự động chuyển WSL2'),
        detail: t('Cửa sổ PowerShell sẽ tự làm toàn bộ các bước và báo khi xong.'),
      });
    } catch (err) {
      wslError = String(err);
    } finally {
      launchingWsl = false;
    }
  }

  async function handleCopyCommand() {
    if (!wslScript) return;
    try {
      await navigator.clipboard.writeText(wslScript.one_line_command);
      copiedCmd = true;
      setTimeout(() => (copiedCmd = false), 1800);
    } catch {}
  }
</script>

{#if open && candidate}
  <div class="fixed inset-0 z-50 bg-black/45 backdrop-blur-sm flex items-center justify-center p-4">
    <div class="rounded-2xl border border-border bg-card p-6 shadow-2xl max-w-lg w-full max-h-full overflow-y-auto space-y-4">
      <div class="flex items-center justify-between">
        <h3 class="font-bold text-base text-primary">
          {t('Chuyển dữ liệu')}: {candidate.name}
        </h3>
        <button type="button" onclick={onClose} class="text-secondary hover:text-primary">
          <X class="h-4 w-4" />
        </button>
      </div>

      <div class="space-y-3 text-sm text-secondary">
        {#if candidate.id === 'wsl2'}
          <p class="text-xs text-primary leading-relaxed">
            {t('WSL2 đang dùng {size} trên ổ C:. GreenC đã tạo sẵn lệnh PowerShell tự động (chỉ xóa bản cũ sau khi sao lưu sang ổ {drive}: thành công).', {
              size: formatSize(candidate.size_bytes),
              drive: targetDrive,
            })}
          </p>

          <div class="space-y-2 rounded-xl border border-border bg-subtle/60 p-3.5">
            <div class="flex items-center justify-between gap-2">
              <label for="wsl-distro-select" class="text-xs font-semibold text-primary">
                {t('Chọn bản Linux (WSL2)')}
              </label>
              {#if loadingWsl}
                <span class="text-[11px] text-secondary">{t('Đang tìm...')}</span>
              {/if}
            </div>

            {#if wslDistros.length > 0}
              <select
                id="wsl-distro-select"
                bind:value={selectedDistro}
                class="w-full px-3 py-2 rounded-lg border border-border bg-card text-xs font-semibold text-primary outline-none"
              >
                {#each wslDistros as d}
                  <option value={d.name}>
                    {d.name}{d.is_docker_internal ? ` (${t('Nội bộ Docker')})` : ''}
                  </option>
                {/each}
              </select>
            {:else}
              <input
                id="wsl-distro-select"
                type="text"
                bind:value={selectedDistro}
                placeholder="Ubuntu"
                class="w-full px-3 py-2 rounded-lg border border-border bg-card text-xs font-mono text-primary outline-none"
              />
            {/if}

            {#if wslError}
              <p class="text-xs text-rose-600 dark:text-rose-400">{wslError}</p>
            {/if}

            {#if !isStoreBuild && wslScript}
              <button
                type="button"
                disabled={launchingWsl}
                onclick={handleLaunchPowershell}
                class="w-full inline-flex items-center justify-center gap-2 px-4 py-2.5 rounded-xl bg-accent hover:bg-accent-hover text-white text-xs font-bold shadow-sm disabled:opacity-50"
              >
                <Play class="h-4 w-4" />
                {launchingWsl
                  ? t('Đang mở PowerShell...')
                  : t('Bấm chạy tự động bằng PowerShell (1 chạm)')}
              </button>
            {/if}

            {#if wslScript}
              <div class="space-y-1 pt-1">
                <div class="flex items-center justify-between text-[11px] text-secondary">
                  <span class="inline-flex items-center gap-1 font-medium">
                    <Terminal class="h-3.5 w-3.5" />
                    {t('Hoặc sao chép 1 dòng lệnh PowerShell')}
                  </span>
                  <button
                    type="button"
                    onclick={handleCopyCommand}
                    class="inline-flex items-center gap-1 text-accent hover:underline font-semibold"
                  >
                    {#if copiedCmd}
                      <Check class="h-3.5 w-3.5" />
                      {t('Đã chép')}
                    {:else}
                      <Copy class="h-3.5 w-3.5" />
                      {t('Sao chép lệnh')}
                    {/if}
                  </button>
                </div>
                <pre class="text-[10px] font-mono bg-card border border-border rounded-lg p-2.5 overflow-x-auto text-primary whitespace-pre-wrap break-all select-all">{wslScript.one_line_command}</pre>
              </div>
            {/if}
          </div>
        {:else if candidate.id === 'zalo'}
          <p class="text-xs text-primary font-medium">
            {t('{app} tự chuyển dữ liệu của mình, {product} chỉ hướng dẫn và kiểm tra lại. Hiện đang dùng {size} trên ổ C:.', {
              app: candidate.name,
              product: 'GreenC',
              size: formatSize(candidate.size_bytes),
            })}
          </p>
          <ol class="list-decimal list-inside space-y-1.5 bg-subtle p-3 rounded-lg text-xs leading-relaxed">
            <li>{t('Mở {app} trên máy tính.', { app: candidate.name })}</li>
            <li>{t('Bấm biểu tượng Cài đặt ở góc dưới bên trái, chọn Dữ liệu.')}</li>
            <li>{t('Chọn Quản lý dữ liệu, bấm Thay đổi ở phần vị trí lưu.')}</li>
            <li>{t('Chọn thư mục {path} rồi bấm Di chuyển.', { path: `${targetDrive}:\\GreenC\\${candidate.name}` })}</li>
            <li>{t('Đợi ứng dụng chuyển xong, quay lại đây và bấm Kiểm tra lại.')}</li>
          </ol>
        {:else}
          <p class="text-xs text-primary font-medium">
            {t('{app} tự chuyển dữ liệu của mình, {product} chỉ hướng dẫn và kiểm tra lại. Hiện đang dùng {size} trên ổ C:.', {
              app: candidate.name,
              product: 'GreenC',
              size: formatSize(candidate.size_bytes),
            })}
          </p>
          <ol class="list-decimal list-inside space-y-1.5 bg-subtle p-3 rounded-lg text-xs leading-relaxed">
            <li>{t('Mở {app} trên máy tính.', { app: candidate.name })}</li>
            <li>{t('Vào phần Cài đặt > Tài nguyên hoặc Lưu trữ.')}</li>
            <li>{t('Chọn vị trí mới trên ổ đĩa {drive}:\\', { drive: targetDrive })}</li>
            <li>{t('Đợi ứng dụng chuyển xong, quay lại đây và bấm Kiểm tra lại.')}</li>
          </ol>
        {/if}
      </div>

      <div class="pt-2 flex items-center justify-end gap-2.5 border-t border-border/60">
        {#if candidate.id !== 'wsl2'}
          <button
            type="button"
            onclick={() => revealInExplorer(candidate.source_path)}
            class="px-3 py-1.5 text-xs font-medium rounded-lg border border-border hover:bg-subtle text-primary transition-colors flex items-center gap-1.5"
          >
            <Folder class="h-3.5 w-3.5" />
            {t('Mở trong Explorer')}
          </button>
        {/if}
        <button
          type="button"
          onclick={onRecheck}
          class="px-3.5 py-1.5 text-xs font-semibold rounded-lg bg-accent text-white hover:bg-accent-hover transition-colors flex items-center gap-1.5"
        >
          <RefreshCw class="h-3.5 w-3.5" />
          {t('Kiểm tra lại')}
        </button>
      </div>
    </div>
  </div>
{/if}
