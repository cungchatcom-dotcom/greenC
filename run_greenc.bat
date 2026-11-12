@echo off
rem Run Green C:\ from source (development). Works from any checkout location.
cd /d "%~dp0"
uv run python main.py
