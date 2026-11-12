from typing import Any, Callable
from PySide6.QtCore import Qt, QThread, QTimer, Signal
from PySide6.QtWidgets import (
    QFrame,
    QHBoxLayout,
    QLabel,
    QProgressBar,
    QPushButton,
    QVBoxLayout,
    QWidget,
)

from greenc.core.task_controller import TaskControl
from greenc.i18n import tr


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
        self.control.set_callback(self._on_progress, min_interval_s=0.016)

    def _on_progress(self, percent: int, title: str, detail: str) -> None:
        self.progressChanged.emit(percent, title, detail)

    def run(self) -> None:
        try:
            result = self._fn(self.control)
            self.taskFinished.emit(result, self.control.is_stopped)
        except Exception as exc:
            self.taskFailed.emit(str(exc))


class TaskProgressCard(QFrame):
    """Compact status bar at the bottom of the window; hidden while idle."""

    startRequested = Signal()  # kept for compatibility; no start button any more

    def __init__(self, parent: QWidget | None = None):
        super().__init__(parent)
        self.setObjectName("StatusBar")
        self._worker: BackgroundTaskWorker | None = None
        lay = QHBoxLayout(self)
        lay.setContentsMargins(20, 8, 20, 8)
        lay.setSpacing(12)
        self.title_label = QLabel(tr("Sẵn sàng"))
        self.title_label.setStyleSheet("font-weight: 600;")
        self.detail_label = QLabel("")
        self.detail_label.setObjectName("Muted")
        self.detail_label.setMinimumWidth(80)
        self.progress_bar = QProgressBar()
        self.progress_bar.setRange(0, 100)
        self.progress_bar.setTextVisible(False)
        self.progress_bar.setFixedWidth(180)
        self.pct_label = QLabel("")
        self.pct_label.setObjectName("Muted")
        self.pct_label.setFixedWidth(40)
        self.btn_pause = QPushButton(tr("Tạm dừng"))
        self.btn_pause.setEnabled(False)
        self.btn_pause.clicked.connect(self._on_toggle_pause)
        self.btn_stop = QPushButton(tr("Dừng"))
        self.btn_stop.setEnabled(False)
        self.btn_stop.clicked.connect(self._on_stop)
        lay.addWidget(self.title_label)
        lay.addWidget(self.detail_label, 1)
        lay.addWidget(self.progress_bar)
        lay.addWidget(self.pct_label)
        lay.addWidget(self.btn_pause)
        lay.addWidget(self.btn_stop)
        self.setVisible(False)

    def attach_worker(self, worker: BackgroundTaskWorker, initial_title: str) -> None:
        self._worker = worker
        self.setVisible(True)
        self.title_label.setText(initial_title)
        self.detail_label.setText("")
        self.pct_label.setText("0%")
        self.progress_bar.setValue(0)
        self.btn_pause.setEnabled(True)
        self.btn_pause.setText(tr("Tạm dừng"))
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
            self.detail_label.setText(
                self.detail_label.fontMetrics().elidedText(
                    detail, Qt.TextElideMode.ElideRight, max(80, self.detail_label.width())
                )
            )
            self.detail_label.setToolTip(detail)

    def _on_toggle_pause(self) -> None:
        if not self._worker:
            return
        ctrl = self._worker.control
        if ctrl.is_paused:
            ctrl.resume()
            self.btn_pause.setText(tr("Tạm dừng"))
        else:
            ctrl.pause()
            self.btn_pause.setText(tr("Tiếp tục"))
            self.title_label.setText(tr("Đã tạm dừng"))

    def _on_stop(self) -> None:
        if not self._worker:
            return
        self._worker.control.stop()
        self.btn_pause.setEnabled(False)
        self.btn_stop.setEnabled(False)
        self.title_label.setText(tr("Đang dừng…"))

    def _finish_ui(self) -> None:
        self.btn_pause.setEnabled(False)
        self.btn_pause.setText(tr("Tạm dừng"))
        self.btn_stop.setEnabled(False)
        QTimer.singleShot(5000, self._hide_if_idle)

    def _hide_if_idle(self) -> None:
        if self._worker is None or not self._worker.isRunning():
            self.setVisible(False)

    def _on_finished(self, _result: object, was_stopped: bool) -> None:
        if was_stopped:
            self.title_label.setText(tr("Đã dừng"))
            self.detail_label.setText("")
        else:
            self.progress_bar.setValue(100)
            self.pct_label.setText("100%")
        self._finish_ui()

    def _on_failed(self, err_msg: str) -> None:
        self.title_label.setText(tr("Có lỗi"))
        self.detail_label.setText(err_msg)
        self.detail_label.setToolTip(err_msg)
        self._finish_ui()
