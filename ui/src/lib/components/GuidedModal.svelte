<!-- ui/src/lib/components/GuidedModal.svelte -->
<script lang="ts">
  import { t, formatSize } from '$lib/i18n.svelte';
  import { revealInExplorer, type RelocationCandidateDto } from '$lib/api';
  import { X, ExternalLink, RefreshCw, Folder } from 'lucide-svelte';

  interface Props {
    open: boolean;
    candidate: RelocationCandidateDto | null;
    targetDrive: string;
    onClose: () => void;
    onRecheck: () => void;
  }

  let { open, candidate, targetDrive, onClose, onRecheck }: Props = $props();
</script>

{#if open && candidate}
  <div class="fixed inset-0 z-50 bg-black/40 backdrop-blur-sm flex items-center justify-center p-4">
    <div class="rounded-xl border border-border bg-card p-6 shadow-2xl max-w-lg w-full max-h-full overflow-y-auto">
      <div class="flex items-center justify-between mb-4">
        <h3 class="font-bold text-base text-primary">
          {t('Hướng dẫn')}: {candidate.name}
        </h3>
        <button onclick={onClose} class="text-secondary hover:text-primary">
          <X class="h-4 w-4" />
        </button>
      </div>

      <div class="space-y-3 text-sm text-secondary">
        <p class="text-primary font-medium">
          {t('{app} tự chuyển dữ liệu của mình, {product} chỉ hướng dẫn và kiểm tra lại. Hiện đang dùng {size} trên ổ C:.', {
            app: candidate.name,
            product: 'GreenC',
            size: formatSize(candidate.size_bytes)
          })}
        </p>

        {#if candidate.id === 'zalo'}
          <ol class="list-decimal list-inside space-y-1.5 bg-subtle p-3 rounded-lg text-xs leading-relaxed">
            <li>{t('Mở {app} trên máy tính.', { app: candidate.name })}</li>
            <li>{t('Bấm biểu tượng Cài đặt ở góc dưới bên trái, chọn Dữ liệu.')}</li>
            <li>{t('Chọn Quản lý dữ liệu, bấm Thay đổi ở phần vị trí lưu.')}</li>
            <li>{t('Chọn thư mục {path} rồi bấm Di chuyển.', { path: `${targetDrive}:\\GreenC\\${candidate.name}` })}</li>
            <li>{t('Đợi ứng dụng chuyển xong, quay lại đây và bấm Kiểm tra lại.')}</li>
          </ol>
        {:else if candidate.id === 'wsl2'}
          <ol class="list-decimal list-inside space-y-1.5 bg-subtle p-3 rounded-lg text-xs leading-relaxed font-mono">
            <li>wsl --shutdown</li>
            <li>wsl --export Ubuntu {targetDrive}:\WSL\ubuntu.tar</li>
            <li>wsl --unregister Ubuntu</li>
            <li>wsl --import Ubuntu {targetDrive}:\WSL\Ubuntu {targetDrive}:\WSL\ubuntu.tar</li>
            <li>del {targetDrive}:\WSL\ubuntu.tar</li>
          </ol>
        {:else}
          <ol class="list-decimal list-inside space-y-1.5 bg-subtle p-3 rounded-lg text-xs leading-relaxed">
            <li>{t('Mở {app} trên máy tính.', { app: candidate.name })}</li>
            <li>{t('Vào phần Cài đặt > Tài nguyên hoặc Lưu trữ.')}</li>
            <li>{t('Chọn vị trí mới trên ổ đĩa {drive}:\\', { drive: targetDrive })}</li>
            <li>{t('Đợi ứng dụng chuyển xong, quay lại đây và bấm Kiểm tra lại.')}</li>
          </ol>
        {/if}
      </div>

      <div class="mt-6 flex items-center justify-end gap-3">
        <button
          onclick={() => revealInExplorer(candidate.source_path)}
          class="px-3 py-1.5 text-xs font-medium rounded-lg border border-border hover:bg-subtle text-primary transition-colors flex items-center gap-1.5"
        >
          <Folder class="h-3.5 w-3.5" />
          {t('Mở trong Explorer')}
        </button>
        <button
          onclick={onRecheck}
          class="px-3 py-1.5 text-xs font-semibold rounded-lg bg-accent text-white hover:bg-accent-hover transition-colors flex items-center gap-1.5"
        >
          <RefreshCw class="h-3.5 w-3.5" />
          {t('Kiểm tra lại')}
        </button>
      </div>
    </div>
  </div>
{/if}
