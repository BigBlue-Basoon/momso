"""Remove only exact installation identities from an IAM policy in memory."""
import copy
import re


def without_accounts(policy, emails):
    if not isinstance(policy, dict) or not isinstance(policy.get('etag'), str) or not policy['etag']:
        raise ValueError('policy_unverified')
    if 'bindings' not in policy:
        return copy.deepcopy(policy)
    bindings = policy.get('bindings', [])
    if not isinstance(bindings, list): raise ValueError('policy_unverified')
    patterns = [re.compile(r'^(?:deleted:)?serviceAccount:' + re.escape(email) + r'(?:\?uid=[0-9]+)?$') for email in emails]
    result = copy.deepcopy(policy)
    retained = []
    for binding in result.get('bindings', []):
        if not isinstance(binding, dict) or not isinstance(binding.get('members'), list):
            raise ValueError('policy_unverified')
        members = binding['members']
        if any(not isinstance(member, str) for member in members): raise ValueError('policy_unverified')
        binding['members'] = [member for member in members if not any(pattern.fullmatch(member) for pattern in patterns)]
        if binding['members']: retained.append(binding)
    result['bindings'] = retained
    return result


def remove_and_verify(transport, resources, emails):
    """Refuse to claim completion if any policy could not be read or verified."""
    for resource in resources:
        policy = transport.read(resource)
        if policy is None: continue
        filtered = without_accounts(policy, emails)
        if filtered != policy:
            transport.write(resource, filtered)
        current = transport.read(resource)
        if current is None or without_accounts(current, emails) != current:
            raise ValueError('iam_bindings_unverified')
