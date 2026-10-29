# usage: python3 tools/dhs_config.py <server>/plugins/DHSupport/config.yml
# Sets DH Support up to only serve stored LODs: no LOD builder (it would load and generate real chunks),
# and no events that delete stored LODs to rebuild them.
import sys
p = sys.argv[1]
out, skipping = [], False
for line in open(p).read().split('\n'):
    if skipping and (line.startswith('  - ') or line.startswith('#  - ')):
        continue
    skipping = False
    if line.startswith('update_events:'):
        line, skipping = 'update_events: []', True
    elif line.startswith('#generate_new_chunks:'):
        line = 'generate_new_chunks: false'
    elif line.strip().startswith('builder_type:'):
        line = line[:len(line) - len(line.lstrip())] + 'builder_type: none'
    out.append(line)
open(p, 'w').write('\n'.join(out))
