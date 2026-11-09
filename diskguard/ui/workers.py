from typing import Any, Callable
from PySide6.QtCore import Qt, QThread, Signal
from PySide6.QtWidgets import (
    QFrame,
    QHBoxLayout,
    QLabel,
    QProgressBar,
    QPushButton,
    QVBoxLayout,
    QWidget,
)

from diskguard.core.task_controller import TaskControl


class BackgroundTaskWorker(QThread):
    """
    Executes a heavy task in a background QThread so the UI never freezes.
    Supports Pause, Resume, Stop and step-by-step live captions via TaskControl.
    """

    progressChanged = Signal(int, str, str)  # percent, step_title, step_detail
    taskFinished = Signal(object, bool)  # result, was_stopped
    taskFailed = Signal(str)

    def __init__(
        self,
        fn: Callable[[TaskControl], Any],
        parent: QWidget | None = None,
    ):
        super().__init__(parent)
        self._fn = fn
        self.control = TaskControl()
        self.control.set_callback(self._on_progress)

    def _on_progress(self, percent: int, title: str, detail: str) -> None:
        self.progressChanged.emit(percent, title, detail)

    def run(self) -> None:
        try:
            result = self._fn(self.control)
            self.taskFinished.emit(result, self.control.is_stopped)
        except Exception as exc:
            self.taskFailed.emit(str(exc))


class TaskProgressCard(QFrame):
    """
    Clean Apple/Google-style progress panel displaying:
    - Current step title (bold)
    - Smooth progress bar (0..100%)
    - Live caption underneath explaining what the background worker is doing
    - Start / Pause / Resume / Stop buttons
    """

    startRequested = Signal()

    def __init__(self, parent: QWidget | None = None):
        super().__init__(parent)
        self.setObjectName("TaskProgressCard")
        self._worker: BackgroundTaskWorker | None = None

        layout = QVBoxLayout(self)
        layout.setContentsMargins(16, 12, 16, 12)
        layout.setSpacing(6)

        top_row = QHBoxLayout()
        self.title_label = QLabel("Hệ thống sẵn sàng")
        self.title_label.setStyleSheet(
            "font-size: 13px; font-weight: 600; color: #1D1D1F;"
        )
        top_row.addWidget(self.title_label, stretch=1)

        self.pct_label = QLabel("")
        self.pct_label.setStyleSheet(
            "font-size: 12px; font-weight: 600; color: #0066CC;"
        )
        top_row.addWidget(self.pct_label)

        self.btn_start = QPushButton("Bắt đầu quét")
        self.btn_start.setObjectName("SecondaryButton")
        self.btn_start.clicked.connect(self.startRequested.emit)

        self.btn_pause = QPushButton("Tạm dừng")
        self.btn_pause.setObjectName("SecondaryButton")
        self.btn_pause.setEnabled(False)
        self.btn_pause.clicked.connect(self._on_toggle_pause)

        self.btn_stop = QPushButton("Dừng")
        self.btn_stop.setObjectName("SecondaryButton")
        self.btn_stop.setEnabled(False)
        self.btn_stop.clicked.connect(self._on_stop)

        top_row.addWidget(self.btn_start)
        top_row.addWidget(self.btn_pause)
        top_row.addWidget(self.btn_stop)
        layout.addLayout(top_row)

        self.progress_bar = QProgressBar()
        self.progress_bar.setRange(0, 100)
        self.progress_bar.setValue(0)
        self.progress_bar.setTextVisible(False)
        self.progress_bar.setFixedHeight(6)
        layout.addWidget(self.progress_bar)

        self.detail_label = QLabel(
            "Các quy tắc bảo vệ dung lượng đang chạy ngầm. Bạn có thể bấm 'Bắt đầu quét' hoặc chuyển sang mục Dọn dẹp Thông minh."
        )
        self.detail_label.setStyleSheet("font-size: 12px; color: #6E6E73;")
        self.detail_label.setWordWrap(True)
        layout.addWidget(self.detail_label)

    def attach_worker(self, worker: BackgroundTaskWorker, initial_title: str) -> None:
        self._worker = worker
        self.title_label.setText(initial_title)
        self.pct_label.setText("0%")
        self.progress_bar.setValue(0)
        self.btn_start.setEnabled(False)
        self.btn_pause.setEnabled(True)
        self.btn_pause.setText("Tạm dừng")
        self.btn_stop.setEnabled(True)

        worker.progressChanged.connect(self.update_progress)
        worker.taskFinished.connect(self._on_finished)
        worker.taskFailed.connect(self._on_failed)

    def update_progress(self, percent: int, title: str, detail: str) -> None:
        self.progress_bar.setValue(percent)
        self.pct_label.setText(f"{percent}%")
        if title:
            self.title_label.setText(title)
        if detail:
            self.detail_label.setText(detail)

    def _on_toggle_pause(self) -> None:
        if not self._worker:
            return
        ctrl = self._worker.control
        if ctrl.is_paused:
            ctrl.resume()
            self.btn_pause.setText("Tạm dừng")
            self.title_label.setText("Đang tiếp tục xử lý...")
        else:
            ctrl.pause()
            self.btn_pause.setText("Tiếp tục")
            self.title_label.setText("Đã tạm dừng — Bấm 'Tiếp tục' để chạy tiếp")

    def _on_stop(self) -> None:
        if not self._worker:
            return
        self._worker.control.stop()
        self.btn_pause.setEnabled(False)
        self.btn_stop.setEnabled(False)
        self.title_label.setText("Đang dừng tác vụ an toàn...")
        self.detail_label.setText("Đang hoàn tất bước hiện tại và giải phóng tài nguyên...")

    def _on_finished(self, _result: object, was_stopped: bool) -> None:
        self.btn_start.setEnabled(True)
        self.btn_pause.setEnabled(False)
        self.btn_pause.setText("Tạm dừng")
        self.btn_stop.setEnabled(False)
        if was_stopped:
            self.title_label.setText("Đã dừng theo yêu cầu")
            self.detail_label.setText("Tác vụ đã dừng an toàn. Bạn có thể bấm 'Bắt đầu quét' để chạy lại.")
        else:
            self.progress_bar.setValue(100)
            self.pct_label.setText("100%")

    def _on_failed(self, err_msg: str) -> None:
        self.btn_start.setEnabled(True)
        self.btn_pause.setEnabled(False)
        self.btn_pause.setText("Tạm dừng")
        self.btn_stop.setEnabled(False)
        self.title_label.setText("Tác vụ gặp sự cố")
        self.detail_label.setText(err_msg)
