from PySide6.QtCore import QByteArray, Qt
from PySide6.QtGui import QIcon, QPainter, QPixmap
from PySide6.QtSvg import QSvgRenderer

from diskguard.ui.theme import BRAND_GREEN, TEXT_MUTED

_PATHS = {
    "eye": '<path d="M1.5 8s2.4-4.5 6.5-4.5S14.5 8 14.5 8s-2.4 4.5-6.5 4.5S1.5 8 1.5 8z"/><circle cx="8" cy="8" r="2"/>',
    "copy": '<rect x="5" y="5" width="8.5" height="8.5" rx="1.5"/><path d="M3.5 10.5H3A1.5 1.5 0 0 1 1.5 9V3A1.5 1.5 0 0 1 3 1.5h6A1.5 1.5 0 0 1 10.5 3v.5"/>',
    "folder": '<path d="M1.5 4.5V12a1 1 0 0 0 1 1h11a1 1 0 0 0 1-1V6a1 1 0 0 0-1-1H8L6.5 3.5h-4a1 1 0 0 0-1 1z"/>',
    "check": '<path d="M3 8.5l3 3 7-7"/>',
    "warning": '<path d="M8 1.8l6.5 11.7h-13z"/><path d="M8 6v3.5M8 11.6v.1"/>',
    "drive": '<rect x="1.5" y="4" width="13" height="8" rx="1.5"/><path d="M11.5 8h.1M4 8h4"/>',
}


def _svg(name: str, color: str) -> bytes:
    return (
        '<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 16 16" fill="none" '
        f'stroke="{color}" stroke-width="1.2" stroke-linecap="round" stroke-linejoin="round">'
        f"{_PATHS[name]}</svg>"
    ).encode("utf-8")


def icon(name: str, color: str = TEXT_MUTED, size: int = 16) -> QIcon:
    renderer = QSvgRenderer(QByteArray(_svg(name, color)))
    scale = 2
    pixmap = QPixmap(size * scale, size * scale)
    pixmap.fill(Qt.GlobalColor.transparent)
    painter = QPainter(pixmap)
    renderer.render(painter)
    painter.end()
    pixmap.setDevicePixelRatio(scale)
    return QIcon(pixmap)


def app_icon_svg() -> bytes:
    """Filled tile used for the window, taskbar, exe and installer icons."""
    return (
        '<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 16 16">'
        f'<rect x="0.5" y="0.5" width="15" height="15" rx="3.5" fill="{BRAND_GREEN}"/>'
        '<g fill="none" stroke="#FFFFFF" stroke-width="1.3" stroke-linecap="round" stroke-linejoin="round">'
        '<rect x="3" y="5" width="10" height="6" rx="1.2"/><path d="M10.4 8h.1M5.2 8h2.8"/></g></svg>'
    ).encode("utf-8")


def render_app_icon(size: int) -> QPixmap:
    renderer = QSvgRenderer(QByteArray(app_icon_svg()))
    pixmap = QPixmap(size, size)
    pixmap.fill(Qt.GlobalColor.transparent)
    painter = QPainter(pixmap)
    painter.setRenderHint(QPainter.RenderHint.Antialiasing)
    renderer.render(painter)
    painter.end()
    return pixmap


def app_icon() -> QIcon:
    result = QIcon()
    for size in (16, 24, 32, 48, 64, 128, 256):
        result.addPixmap(render_app_icon(size))
    return result
