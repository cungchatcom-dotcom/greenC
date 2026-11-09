import threading
import time
from typing import Callable


class TaskControl:
    """
    Thread-safe controller for long-running background operations.
    Supports Start, Pause, Resume, Stop and step-by-step progress reporting
    with 60Hz UI signal coalescing so the GUI thread never stalls.
    """

    def __init__(self, min_report_interval_s: float = 0.0) -> None:
        self._pause_event = threading.Event()
        self._pause_event.set()  # Not paused initially
        self._stop_flag = False
        self._lock = threading.Lock()
        self._callback: Callable[[int, str, str], None] | None = None
        self._last_report_ts: float = 0.0
        self._min_interval_s = min_report_interval_s

    def set_callback(
        self,
        callback: Callable[[int, str, str], None],
        min_interval_s: float = 0.0,
    ) -> None:
        with self._lock:
            self._callback = callback
            self._min_interval_s = min_interval_s

    def reset(self) -> None:
        with self._lock:
            self._stop_flag = False
            self._last_report_ts = 0.0
            self._pause_event.set()

    def pause(self) -> None:
        self._pause_event.clear()

    def resume(self) -> None:
        self._pause_event.set()

    def stop(self) -> None:
        with self._lock:
            self._stop_flag = True
            self._pause_event.set()

    @property
    def is_paused(self) -> bool:
        return not self._pause_event.is_set()

    @property
    def is_stopped(self) -> bool:
        with self._lock:
            return self._stop_flag

    def wait_if_paused(self) -> bool:
        """
        Blocks while paused. Returns False if the task was stopped,
        or True if execution should continue.
        """
        self._pause_event.wait()
        return not self.is_stopped

    def report(self, percent: int, step_title: str, detail: str = "") -> bool:
        """
        Reports progress (0..100), step title, and detailed sub-step caption,
        then waits if paused. Returns False if stopped.
        """
        pct = max(0, min(100, int(percent)))
        now = time.perf_counter()
        cb = None
        with self._lock:
            if (
                self._callback is not None
                and (
                    pct in (0, 100)
                    or self._min_interval_s <= 0.0
                    or (now - self._last_report_ts) >= self._min_interval_s
                )
            ):
                cb = self._callback
                self._last_report_ts = now
        if cb is not None:
            cb(pct, step_title, detail)
        return self.wait_if_paused()

