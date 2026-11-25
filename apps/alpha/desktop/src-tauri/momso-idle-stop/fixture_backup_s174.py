"""Slow local backup agent for the S174 real-server test."""
import time
print('momso-backup-v3:snapshot-ready', flush=True)
time.sleep(4)
print('momso-backup-v3:uploaded', flush=True)
