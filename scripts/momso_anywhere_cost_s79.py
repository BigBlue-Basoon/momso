"""Illustrative Seoul monthly cost; see docs/development/momso-anywhere-cost-s79.md."""

import argparse


def estimate(vm_hours, gateway_seconds, *, disk_gib=20, month_hours=720,
             nat_vm_hours=None, nat_ip_hours=None, nat_gib=1, korea_gib=1,
             requests=30000, secret_versions=3, secret_access=3000):
    if not 0 <= vm_hours <= month_hours or gateway_seconds < 0:
        raise ValueError('invalid usage hours')
    nat_vm_hours = vm_hours if nat_vm_hours is None else nat_vm_hours
    nat_ip_hours = month_hours if nat_ip_hours is None else nat_ip_hours
    values = [disk_gib, month_hours, nat_vm_hours, nat_ip_hours, nat_gib,
              korea_gib, requests, secret_versions, secret_access]
    if (any(value < 0 for value in values) or nat_vm_hours > month_hours
            or nat_ip_hours > month_hours or korea_gib > 1024):
        raise ValueError('invalid resource usage')

    # Cloud Run free allowances are dollar credits at Iowa active rates.
    run_cpu = max(0, gateway_seconds * 0.0000336 - 180000 * 0.000024)
    run_memory = max(0, gateway_seconds * 0.5 * 0.0000035 - 360000 * 0.0000025)
    return {
        'vm': vm_hours * 0.02149143,
        'disk': disk_gib * 0.052 * month_hours / 730,
        'nat_vm': nat_vm_hours * 0.0014,
        'nat_ip': nat_ip_hours * 0.005,
        'nat_processing': nat_gib * 0.045,
        'internet_korea': korea_gib * 0.19,
        'run_cpu': run_cpu,
        'run_memory': run_memory,
        'run_requests': max(0, requests - 2000000) / 1000000 * 0.40,
        'secret_versions': max(0, secret_versions - 6) * 0.06 * month_hours / 730,
        'secret_access': max(0, secret_access - 10000) / 10000 * 0.03,
    }


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--nat-ip-hours', type=float, default=720)
    parser.add_argument('--nat-gib', type=float, default=1)
    parser.add_argument('--korea-gib', type=float, default=1)
    args = parser.parse_args()
    for label, vm_hours, usage_hours in [('1h/day', 30, 30), ('8h/day', 240, 240), ('always-on', 720, 720)]:
        parts = estimate(vm_hours, usage_hours * 300, nat_ip_hours=args.nat_ip_hours,
                         nat_gib=args.nat_gib, korea_gib=args.korea_gib)
        print(f'{label}: ${sum(parts.values()):.2f}')
        for name, value in parts.items():
            print(f'  {name}: ${value:.4f}')


if __name__ == '__main__':
    main()
