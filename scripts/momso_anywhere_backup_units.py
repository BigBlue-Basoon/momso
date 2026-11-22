"""Unit templates; the installer must opt in only after provisioning succeeds."""


def unit_files():
    service = """[Unit]
Description=momso encrypted backup
Wants=network-online.target
After=network-online.target

[Service]
Type=oneshot
User=momso
EnvironmentFile=/etc/momso-backup.env
RuntimeDirectory=momso-backup
RuntimeDirectoryMode=0700
ExecStart=/usr/bin/python3 /opt/momso/momso_anywhere_backup_agent.py backup
NoNewPrivileges=true
PrivateTmp=true
"""
    timer = """[Unit]
Description=Daily momso encrypted backup

[Timer]
OnCalendar=daily
Persistent=true
Unit=momso-backup.service

[Install]
WantedBy=timers.target
"""
    # The caller must wait for this service before initiating an idle stop;
    # a shutdown hook alone can be killed by the VM provider's timeout.
    return {"momso-backup.service": service, "momso-backup.timer": timer}
