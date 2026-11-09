import sys
from PySide6.QtWidgets import QApplication
from diskguard.ui.main_window import MainWindow


def main() -> int:
    app = QApplication(sys.argv)
    app.setApplicationName("DiskGuard")
    window = MainWindow(auto_scan_on_init=True)
    window.show()
    return app.exec()


if __name__ == "__main__":
    raise SystemExit(main())
