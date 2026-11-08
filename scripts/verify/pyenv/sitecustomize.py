# Imported by every Python the verification suites start on Windows (lib.sh
# puts this folder on PYTHONPATH there): print() writes "\n", not "\r\n".
#
# On Windows, Python's text output turns each "\n" into "\r\n", even into a
# pipe. Bash's $(...) drops the "\n" and keeps the "\r", so every value a suite
# read through Python carried a CR, and joining several of them gave
# "0\r,0\r,0\r," where a check wanted "0,0,0,".
import sys

for stream in (sys.stdout, sys.stderr):
    try:
        stream.reconfigure(newline="\n")
    except (AttributeError, ValueError):
        pass
