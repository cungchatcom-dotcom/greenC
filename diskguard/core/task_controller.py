import threading
from typing import Callable


class TaskControl:
    """
    Thread-safe controller for long-running background operations.
    Supports Start, Pause, Resume, Stop and step-by-step progress reporting
    with clear user-facing descriptions of the current operation.
    """

    def __init__(self) -> None:
        self._pause_event = threading.Event()
        self._pause_event.set()  # Not paused initially
        self._stop_flag = False
        self._lock = threading.Lock()
        self._callback: Callable[[int, str, str], None] | None = None

    def set_callback(self, callback: Callable[[int, str, str], None]) -> None:
        with self._lock:
            self._callback = callback

    def reset(self) -> None:
        with self._lock:
            self._stop_flag = False
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
        cb = None
        with self._lock:
            cb = self._callback
        if cb is not None:
            cb(max(0, min(100, int(percent))), step_title, detail)
        return self.wait_if_paused()

