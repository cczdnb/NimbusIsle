@echo off
rem Portable package: the diagnostics script is embedded in yunyu.exe.
start "" "%~dp0yunyu.exe" --collect-diagnostics
