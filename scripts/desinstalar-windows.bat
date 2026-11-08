@echo off
rem Desinstala el comando buscatxt del usuario actual (doble clic).
powershell -NoProfile -ExecutionPolicy Bypass -File "%~dp0instalar-windows.ps1" -Desinstalar
pause
