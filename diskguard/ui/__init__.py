def format_bytes(num_bytes: int | float) -> str:
    val = float(num_bytes)
    sign = "-" if val < 0 else ""
    val = abs(val)
    for unit in ("B", "KB", "MB", "GB", "TB"):
        if val < 1024.0 or unit == "TB":
            if unit in ("B", "KB"):
                return f"{sign}{val:.0f} {unit}"
            return f"{sign}{val:.2f} {unit}"
        val /= 1024.0
    return f"{sign}{val:.2f} TB"

