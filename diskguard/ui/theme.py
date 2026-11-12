from PySide6.QtWidgets import QWidget

ACCENT = "#0067C0"
ACCENT_HOVER = "#005BA9"
ACCENT_PRESSED = "#004F94"
BRAND_GREEN = "#107C10"
DANGER = "#C42B1C"
WARNING_BG = "#FFF4E5"
WARNING_FG = "#8A5300"
TEXT = "#1B1B1B"
TEXT_MUTED = "#616161"
BG_SHELL = "#F3F3F3"
BG_CONTENT = "#FAFAFA"
SURFACE = "#FFFFFF"
BORDER = "#E5E5E5"
BORDER_STRONG = "#D1D1D1"
NAV_HOVER = "#EAEAEA"
NAV_SELECTED = "#E5E5E5"
SELECTION = "#E5F0FB"
FONT_FAMILY = '"Segoe UI Variable Text", "Segoe UI"'
MONO_FAMILY = '"Cascadia Mono", Consolas'

QSS = f"""
* {{ font-family: {FONT_FAMILY}; font-size: 13px; color: {TEXT}; }}
QMainWindow, QDialog {{ background: {BG_CONTENT}; }}
QFrame#Sidebar {{ background: {BG_SHELL}; border: none; border-right: 1px solid {BORDER}; }}
QWidget#Content, QWidget#PageBody {{ background: {BG_CONTENT}; }}
QScrollArea#PageScroll {{ background: {BG_CONTENT}; border: none; }}
QLabel#Brand {{ font-size: 17px; font-weight: 600; padding: 2px 10px 16px 10px; }}
QLabel#AuthorBlock {{ color: {TEXT_MUTED}; font-size: 12px; padding: 10px 10px 2px 10px; border-top: 1px solid {BORDER}; }}
QPushButton#NavButton {{ background: transparent; border: none; border-radius: 4px; padding: 8px 12px; text-align: left; }}
QPushButton#NavButton:hover {{ background: {NAV_HOVER}; }}
QPushButton#NavButton:checked {{ background: {NAV_SELECTED}; font-weight: 600; border-left: 3px solid {ACCENT}; padding-left: 9px; }}
QLabel#PageTitle {{ font-size: 20px; font-weight: 600; }}
QLabel#CardTitle {{ font-size: 14px; font-weight: 600; }}
QLabel#Muted {{ color: {TEXT_MUTED}; }}
QLabel#PathText {{ font-family: {MONO_FAMILY}; font-size: 12px; color: {TEXT_MUTED}; }}
QLabel#Tag {{ background: {SELECTION}; color: #0B5394; border-radius: 9px; padding: 1px 8px; font-size: 12px; }}
QLabel#TagWarning {{ background: {WARNING_BG}; color: {WARNING_FG}; border-radius: 9px; padding: 1px 8px; font-size: 12px; }}
QLabel#TagSuccess {{ background: #E9F5E9; color: {BRAND_GREEN}; border-radius: 9px; padding: 1px 8px; font-size: 12px; }}
QLabel#TagNeutral {{ background: #F0F0F0; color: {TEXT_MUTED}; border-radius: 9px; padding: 1px 8px; font-size: 12px; }}
QFrame#Card {{ background: {SURFACE}; border: 1px solid {BORDER}; border-radius: 6px; }}
QFrame#Banner {{ background: {WARNING_BG}; border: 1px solid #F3D9B1; border-radius: 6px; }}
QPushButton {{ background: {SURFACE}; border: 1px solid {BORDER_STRONG}; border-radius: 4px; padding: 5px 14px; }}
QPushButton:hover {{ background: #F6F6F6; }}
QPushButton:pressed {{ background: #EDEDED; }}
QPushButton:disabled {{ color: #A0A0A0; background: {BG_CONTENT}; border-color: {BORDER}; }}
QPushButton#PrimaryButton {{ background: {ACCENT}; color: #FFFFFF; border: 1px solid {ACCENT}; font-weight: 600; }}
QPushButton#PrimaryButton:hover {{ background: {ACCENT_HOVER}; }}
QPushButton#PrimaryButton:pressed {{ background: {ACCENT_PRESSED}; }}
QPushButton#PrimaryButton:disabled {{ background: #9DC1E4; border-color: #9DC1E4; color: #FFFFFF; }}
QToolButton#IconButton {{ background: transparent; border: none; border-radius: 4px; padding: 3px; }}
QToolButton#IconButton:hover {{ background: #F0F0F0; }}
QToolButton#IconButton:checked {{ background: {SELECTION}; }}
QTableWidget {{ background: {SURFACE}; border: none; gridline-color: transparent; selection-background-color: {SELECTION}; selection-color: {TEXT}; }}
QTableWidget::item {{ padding: 4px 8px; border-bottom: 1px solid #F0F0F0; }}
QHeaderView::section {{ background: {SURFACE}; color: {TEXT_MUTED}; font-weight: 600; font-size: 12px; padding: 6px 8px; border: none; border-bottom: 1px solid {BORDER}; }}
QProgressBar {{ background: {BORDER}; border: none; border-radius: 3px; max-height: 6px; min-height: 6px; }}
QProgressBar::chunk {{ background: {ACCENT}; border-radius: 3px; }}
QProgressBar#DangerBar::chunk {{ background: {DANGER}; }}
QComboBox, QLineEdit, QTextEdit, QSpinBox, QDoubleSpinBox {{ background: {SURFACE}; border: 1px solid {BORDER_STRONG}; border-radius: 4px; padding: 4px 8px; }}
QComboBox:focus, QLineEdit:focus, QTextEdit:focus, QSpinBox:focus, QDoubleSpinBox:focus {{ border-bottom: 2px solid {ACCENT}; }}
QFrame#DriveOption {{ background: {SURFACE}; border: 1px solid {BORDER}; border-radius: 6px; }}
QFrame#DriveOption[selected="true"] {{ border: 2px solid {ACCENT}; background: #F6FAFF; }}
QFrame#StatusBar {{ background: {SURFACE}; border-top: 1px solid {BORDER}; }}
QFrame#DialogFooter {{ background: #F7F7F7; border-top: 1px solid {BORDER}; }}
QLabel#Toast {{ background: #1B1B1B; color: #FFFFFF; border-radius: 4px; padding: 6px 12px; }}
QCheckBox::indicator {{ width: 16px; height: 16px; }}
"""


def apply_theme(target) -> None:
    """Apply the single app stylesheet to a QApplication or a top-level widget."""
    target.setStyleSheet(QSS)


def repolish(widget: QWidget) -> None:
    widget.style().unpolish(widget)
    widget.style().polish(widget)
    widget.update()
