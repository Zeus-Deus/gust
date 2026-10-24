#!/usr/bin/env python3
"""Real Gust/Wayland sharing flow with synthetic Tailscale; no host input or sharing.

Owns a private headless labwc, session bus, HOME/XDG roots and only its own PIDs.
This qualifies app integration, never physical iOS devices or live tailnet policy.
"""
import argparse
import fcntl
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import time


TAILSCALE = r'''#!/usr/bin/python3
import json, os, pathlib, shutil, sys, time
r = pathlib.Path(os.environ['GUST_SHARING_FIXTURE'])
a = sys.argv[1:]
with (r/'calls.jsonl').open('a') as f: f.write(json.dumps(a)+'\n')
if a == ['status', '--json']:
    print(json.dumps({'BackendState':'Running','CurrentTailnet':{'MagicDNSEnabled':True},
        'Self':{'HostName':'desktop','DNSName':'desktop.fixture.test.'},'Peer':{
        'phone':{'HostName':'iphone-15','DNSName':'iphone-15.fixture.test.','TailscaleIPs':['100.64.0.2'],'OS':'iOS','Online':True},
        'pad':{'HostName':'ipad','DNSName':'ipad.fixture.test.','TailscaleIPs':['100.64.0.3'],'OS':'iPadOS','Online':False}}}))
elif a == ['file','cp','--targets']:
    print('100.64.0.2\tiphone-15\n100.64.0.3\tipad\toffline')
elif a == ['drive','list']:
    shares = json.loads((r/'shares.json').read_text())
    nw = max([4]+[len(n) for n in shares]); pw = max([4]+[len(p) for p in shares.values()])
    print(f'{"name":<{nw}}    {"path":<{pw}}    as')
    print(f'{"-"*nw}    {"-"*pw}    --')
    for n,p in shares.items(): print(f'{n:<{nw}}    {p:<{pw}}    fixture')
elif a[:2] == ['drive','share']:
    shares = json.loads((r/'shares.json').read_text()); shares[a[2]] = a[3]
    (r/'shares.json').write_text(json.dumps(shares))
elif a[:2] == ['drive','unshare']:
    shares = json.loads((r/'shares.json').read_text()); shares.pop(a[2],None)
    (r/'shares.json').write_text(json.dumps(shares))
elif a[:2] == ['file','cp']:
    if (r/'mode').read_text() == 'slow': time.sleep(30)
    assert a[2] == '--name' and a[4] == '--' and a[6] == '100.64.0.2:'
    shutil.copyfile(a[5],r/'sent'/a[3])
elif a[:2] == ['file','get']:
    assert a[2] == '--conflict=rename'
    dest = pathlib.Path(a[3])
    # The real CLI owns conflict-safe publication; the stand-in writes only
    # synthetic bytes into Gust's fresh held batch directory.
    with (dest/'from-phone.txt').open('x') as f: f.write('synthetic phone payload\n')
    if (r/'mode').read_text() == 'receive-fail': sys.exit(1)
else: sys.exit(1)
'''


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, required=True)
    parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    binary = args.binary.resolve()
    out = args.output.resolve()
    for tool in ['labwc', 'wlr-randr', 'wtype', 'grim', 'dbus-daemon']:
        if not shutil.which(tool):
            print(f'SKIP: missing {tool}'); return 77
    lease = open('/tmp/gust-qa.lock', 'a')
    try:
        fcntl.flock(lease, fcntl.LOCK_EX | fcntl.LOCK_NB)
    except BlockingIOError:
        print('SKIP: desktop QA lease busy'); return 75
    out.mkdir(parents=True, exist_ok=False)
    home, runtime, config, stubs = (out/n for n in ['home', 'run', 'config', 'stubs'])
    for path in [home, runtime, config/'labwc', stubs, out/'sent', home/'Documents']:
        path.mkdir(parents=True, mode=0o700, exist_ok=True)
    for name in ['.config', '.local/share', '.local/state', '.cache']:
        (home/name).mkdir(parents=True, mode=0o700, exist_ok=True)
    (home/'Documents/sample.txt').write_text('synthetic desktop payload\n')
    (home/'Documents/z-other.txt').write_text('another synthetic file\n')
    (out/'shares.json').write_text('{}')
    (out/'mode').write_text('normal')
    (stubs/'tailscale').write_text(TAILSCALE)
    for name in ['ssh', 'ssh-keyscan', 'sshfs', 'pkexec', 'sudo', 'udisksctl', 'systemctl', 'xdg-open', 'xdg-terminal-exec', 'rclone', 'zoxide']:
        (stubs/name).write_text('#!/bin/sh\nprintf "%s\\n" "$0" >> "$GUST_SHARING_FIXTURE/blocked.log"\nexit 1\n')
    (stubs/'lsblk').write_text('#!/bin/sh\necho \'{"blockdevices":[]}\'\n')
    for path in stubs.iterdir(): path.chmod(0o755)
    (config/'labwc/rc.xml').write_text('''<?xml version="1.0"?>
<labwc_config><windowRules><windowRule identifier="*"><action name="Maximize"/></windowRule></windowRules></labwc_config>''')
    (config/'labwc/autostart').write_text('printf "%s/%s" "$XDG_RUNTIME_DIR" "$WAYLAND_DISPLAY" > "$GUST_SHARING_FIXTURE/display"\n')
    env = {k:v for k,v in os.environ.items() if k in ['PATH', 'LANG', 'LC_ALL', 'XDG_DATA_DIRS']}
    env.update(HOME=str(home), XDG_RUNTIME_DIR=str(runtime), XDG_CONFIG_HOME=str(config),
               XDG_DATA_HOME=str(home/'.local/share'), XDG_STATE_HOME=str(home/'.local/state'),
               XDG_CACHE_HOME=str(home/'.cache'), GUST_SHARING_FIXTURE=str(out),
               WLR_BACKENDS='headless', WLR_LIBINPUT_NO_DEVICES='1', WLR_HEADLESS_OUTPUTS='1',
               PATH=f'{stubs}:/usr/bin:/bin', QT_QPA_PLATFORM='wayland', QT_QUICK_BACKEND='software',
               GIO_USE_VFS='local', GVFS_DISABLE_FUSE='1', GUST_QA_STATE=str(out/'state.json'),
               GUST_OMARCHY_STATE=str(Path.home()/'.local/state/omarchy'))
    processes = []
    checks = []
    logs = []
    def spawn(argv, log_name, process_env):
        log = (out/log_name).open('w'); logs.append(log)
        p = subprocess.Popen(argv, env=process_env, stdout=log, stderr=subprocess.STDOUT, start_new_session=True)
        processes.append(p); return p
    def run(argv):
        return subprocess.run(argv, env=env, check=True, capture_output=True, text=True, timeout=10)
    def state():
        try: return json.loads((out/'state.json').read_text())
        except (OSError, ValueError): return {}
    def wait(predicate, label, timeout=15):
        end = time.monotonic()+timeout
        while time.monotonic() < end:
            if predicate(): return
            time.sleep(.1)
        raise AssertionError(label)
    def check(condition, label):
        if not condition: raise AssertionError(label)
        checks.append(label); print(f'PASS: {label}', flush=True)
    def key(name):
        run(['wtype', '-k', name]); time.sleep(.15)
    def palette(text):
        key('colon'); wait(lambda: state().get('paletteOpen'), 'palette opened')
        run(['wtype', text]); time.sleep(.25); key('Return')
    def ready():
        wait(lambda: bool(state().get('sharingState', {}).get('default_path')) and not state().get('sharingChecking') and not state().get('sharingBusy'), 'sharing ready')
    def button(name):
        for _ in range(25):
            if state().get('sharingFocus') == name: key('Return'); return
            key('Tab')
        raise AssertionError(f'keyboard could not reach {name}')
    def calls():
        path = out/'calls.jsonl'
        return [json.loads(s) for s in path.read_text().splitlines()] if path.exists() else []
    def shot(name):
        time.sleep(.2); run(['grim', str(out/f'{name}.png')])
    result = 1
    try:
        bus = subprocess.Popen(['dbus-daemon', '--session', '--nofork', '--print-address=1', '--nopidfile'], env=env, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL)
        processes.append(bus)
        env['DBUS_SESSION_BUS_ADDRESS'] = bus.stdout.readline().decode().strip()
        spawn(['labwc'], 'labwc.log', env)
        wait(lambda: (out/'display').exists(), 'private compositor started')
        env['WAYLAND_DISPLAY'] = (out/'display').read_text()
        run(['wlr-randr', '--output', 'HEADLESS-1', '--custom-mode', '1280x1100'])
        env['XDG_CONFIG_HOME'] = str(home/'.config')
        app = spawn([str(binary), str(home/'Documents')], 'gust.log', env)
        wait(lambda: state().get('model',{}).get('settled'), 'native Gust started', 30)
        wait(lambda: len(state().get('places',{}).get('mobile',[])) == 2, 'mobile discovery completed')
        # Listing publication can precede the first mapped/focused frame.
        time.sleep(.8)
        for _ in range(6):
            key('End')
            if state().get('model',{}).get('cursor') == 1: break
            time.sleep(.3)
        check(state().get('model',{}).get('cursor') == 1, 'private compositor keyboard reaches Gust')
        key('Home')
        check(not state()['places']['computers'], 'iPhone and iPad are not advertised as SSH computers')
        palette('connect'); wait(lambda: state().get('connectOpen'), 'Connect opened')
        rows = state().get('connectRows', [])
        check(all(r.get('detail') == 'Phone or tablet · send and get files' for r in rows if r.get('kind') == 'mobile'),
              'Connect describes phones and tablets without guessing the model')
        shot('01-apple-devices')
        wait(lambda: state().get('connectSelection') == 'mobile:iphone-15', 'Connect selects the first device')
        check(state().get('connectSelection') == 'mobile:iphone-15', 'Connect opens with a device ready for Enter')
        key('Return')
        wait(lambda: state().get('sharingOpen'), 'device sheet opened from Connect'); ready()
        check(state().get('sharingTitle') == 'iphone-15' and state().get('sharingOnly') == '' and not state().get('connectOpen'),
              'a phone row opens one sheet titled with that device name')
        check(not any(c[:2] in [['drive','share'],['file','get']] or (c[:2] == ['file','cp'] and c[2:] != ['--targets']) for c in calls()),
              'opening the sheet sends, publishes and receives nothing')
        check('sendTo:100.64.0.2' not in state().get('sharingPoints', {}), 'without a selection the sheet offers no send button')
        shot('02-device-sheet')
        check(not state().get('sharingHelpOpen'), 'setup help stays collapsed in the default sharing view')
        button('deviceSharingHelp'); wait(lambda: state().get('sharingHelpOpen'), 'sharing help opened')
        shot('02-sharing-help')
        button('deviceSharingHelp'); wait(lambda: not state().get('sharingHelpOpen'), 'sharing help hidden')
        run(['wlr-randr', '--output', 'HEADLESS-1', '--custom-mode', '520x420'])
        time.sleep(.2); key('Tab')
        wait(lambda: state().get('windowWidth') == 520, 'sharing at a narrow window')
        shot('02-sharing-narrow')
        run(['wlr-randr', '--output', 'HEADLESS-1', '--custom-mode', '1280x1100'])
        time.sleep(.2); key('Tab')
        wait(lambda: state().get('windowWidth') == 1280, 'restored desktop size')
        button('publishDeviceFolder'); wait(lambda: state().get('sharingConfirmOpen'), 'sharing asks for confirmation')
        check(state().get('sharingConfirmTitle') == 'Let devices open this folder?' and 'other people' in state().get('sharingConfirmBody', ''),
              'turning folder access on names who can reach it before anything changes')
        shot('03-sharing-confirmation'); key('Escape')
        check(json.loads((out/'shares.json').read_text()) == {}, 'Cancel publishes no folder')
        button('publishDeviceFolder'); wait(lambda: state().get('sharingConfirmOpen'), 'second confirmation')
        key('Tab'); key('Return'); ready()
        wait(lambda: state()['sharingState'].get('shared_path') == str(home/'Gust Shared'), 'published mapping confirmed')
        check(state().get('sharingLastAction') == 'publish' and not state().get('sharingFailed'), 'folder access reports On in its own row')
        (home/'Gust Shared/keep.txt').write_text('keep this file\n')
        shot('04-folder-on')
        button('publishDeviceFolder'); wait(lambda: state().get('sharingConfirmOpen'), 'stop confirmation')
        check(state().get('sharingConfirmTitle') == 'Turn off folder access?' and 'All files stay' in state().get('sharingConfirmBody', ''),
              'turning folder access off says files stay')
        # An administrator changes the mapping while the question is open.
        changed = str(home/'Documents')
        (out/'shares.json').write_text(json.dumps({'gust':changed}))
        key('Tab'); key('Return'); ready()
        check(state()['sharingFailed'] and json.loads((out/'shares.json').read_text())['gust'] == changed,
              'a changed share mapping is refused without unsharing the replacement')
        button('publishDeviceFolder'); wait(lambda: state().get('sharingConfirmOpen'), 'updated stop confirmation')
        key('Tab'); key('Return'); ready()
        check(json.loads((out/'shares.json').read_text()) == {} and (home/'Gust Shared/keep.txt').read_text() == 'keep this file\n',
              'stopping the displayed share keeps its files')
        button('receiveDeviceFiles'); ready()
        batches = list((home/'Gust Inbox').glob('Received-*/from-phone.txt'))
        check(len(batches) == 1 and batches[0].read_text() == 'synthetic phone payload\n', 'incoming files reach the private inbox')
        check(state().get('sharingLastAction') == 'receive' and 'openDeviceInbox' in state().get('sharingPoints', {}),
              'the result and a way to the files appear in the Get files row')
        shot('07-files-received')
        (out/'mode').write_text('receive-fail')
        button('receiveDeviceFiles'); ready()
        check(state()['sharingFailed'] and 'may already' in state()['sharingMessage'] and len(list((home/'Gust Inbox').glob('Received-*/from-phone.txt'))) == 2,
              'partial receive failure retains delivered bytes and explains uncertainty')
        check(batches[0].read_text() == 'synthetic phone payload\n', 'repeated receiving preserves previous same-named files')
        (out/'mode').write_text('normal')
        button('receiveDeviceFiles'); ready()
        button('openDeviceInbox'); wait(lambda: not state().get('sharingOpen'), 'sheet closed to show received files')
        wait(lambda: state().get('activeTabPath', '').startswith(str(home/'Gust Inbox/Received-')), 'received batch shown')
        check(state().get('activeTabPath', '').startswith(str(home/'Gust Inbox/Received-')), 'Show files opens the batch that just arrived')
        shot('08-received-batch')
        key('BackSpace'); key('BackSpace')
        wait(lambda: state().get('activeTabPath') == str(home), 'back home')
        run(['wtype', 'Documents']); time.sleep(.25); key('Return')
        wait(lambda: state().get('activeTabPath') == str(home/'Documents') and state().get('model',{}).get('settled'), 'back in Documents')
        time.sleep(.3)
        run(['wtype', 'sample']); time.sleep(.25)
        palette('Send a copy'); wait(lambda: state().get('sharingOpen'), 'send picker'); ready()
        check(state().get('sharingOnly') == 'send', 'a file action opens only the Send row')
        shot('05-send-picker')
        button('sendTo:100.64.0.2'); ready()
        check((out/'sent/sample.txt').read_bytes() == (home/'Documents/sample.txt').read_bytes(), 'Taildrop sends the captured selection and preserves its source')
        (out/'mode').write_text('slow')
        button('sendTo:100.64.0.2'); wait(lambda: state().get('sharingBusy'), 'slow send started')
        button('closeDeviceSharing')
        run(['wtype','-M','alt','-k','F4','-m','alt'])
        wait(lambda: state().get('confirmOpen'), 'window close guard')
        check(state().get('confirmTitle') == 'File sharing is still running' and app.poll() is None, 'window close protects an active sharing worker')
        key('Escape')
        palette('Share with iPhone'); wait(lambda: state().get('sharingOpen'), 'in-flight sharing reopened')
        button('cancelDeviceSharing'); ready()
        check(state()['sharingFailed'] and 'may also have arrived' in state()['sharingMessage'], 'cancel acknowledges stopping and retains delivery uncertainty')
        check(state().get('hintStatus') != 'Transfer continues', 'a stopped send clears the obsolete transfer-continues hint')
        shot('05-transfer-stopped')
        (out/'mode').write_text('normal')
        button('closeDeviceSharing')
        sent_before = sum(c[:2] == ['file','cp'] and c[2:] != ['--targets'] for c in calls())
        run(['wtype', '-M', 'ctrl', '-k', 'a', '-m', 'ctrl']); wait(lambda: state().get('selCount') == 2, 'two files selected')
        palette('Share with iPhone'); wait(lambda: state().get('sharingOpen'), 'whole sheet with a selection'); ready()
        check(state().get('sharingOnly') == '' and 'sendTo:100.64.0.2' in state().get('sharingPoints', {})
              and sum(c[:2] == ['file','cp'] and c[2:] != ['--targets'] for c in calls()) == sent_before,
              'an explicit selection appears in the Send row without sending it')
        shot('05-sheet-with-selection')
        button('closeDeviceSharing')
        check(not any(c[:2] in [['up'],['set']] for c in calls()), 'Gust never changes Tailscale account or policy settings')
        blocked = (out/'blocked.log').read_text() if (out/'blocked.log').exists() else ''
        check('ssh-keyscan' not in blocked and 'sshfs' not in blocked and 'xdg-open' not in blocked,
              'sharing never attempts SSH or opens incoming files')
        # Display escaping deliberately makes these names look alike. The
        # selected newline path must be refused, never replaced by its label.
        app.terminate(); app.wait(timeout=5)
        control = home/'two\nlines'; literal = home/'two<U+000A>lines'
        control.mkdir(); literal.mkdir()
        (literal/'keep.txt').write_text('do not share this neighbour\n')
        for n in ['a.txt', 'b.txt']: (control/n).write_text('synthetic bytes\n')
        (out/'state.json').unlink()
        (out/'mode').write_text('normal')
        before = sum(c[:2] == ['drive','share'] for c in calls())
        app = spawn([str(binary), str(control)], 'gust-byte-path.log', env)
        wait(lambda: state().get('model',{}).get('settled'), 'byte-path Gust started', 30)
        time.sleep(.8)
        for _ in range(6):
            key('End')
            if state().get('model',{}).get('cursor') == 1: break
            time.sleep(.3)
        palette('Share this folder'); wait(lambda: state().get('sharingOpen'), 'chosen folder sharing'); ready()
        check(state().get('sharingOnly') == 'folder', 'a folder action opens only folder access, also found by "share folder"')
        shot('06-folder-only')
        button('publishDeviceFolder'); wait(lambda: state().get('sharingConfirmOpen'), 'chosen folder confirmation')
        key('Tab'); key('Return'); ready()
        check(state()['sharingFailed'] and sum(c[:2] == ['drive','share'] for c in calls()) == before
              and json.loads((out/'shares.json').read_text()) == {}
              and (literal/'keep.txt').read_text() == 'do not share this neighbour\n',
              'a control-byte folder is refused without sharing its lookalike display path')
        shot('06-byte-path-refused')
        result = 0
    except Exception as e:
        print(f'FAIL: {e}', file=sys.stderr)
        (out/'failure.txt').write_text(str(e)+'\n')
        if env.get('WAYLAND_DISPLAY'):
            try: shot('failure')
            except Exception: pass
    finally:
        for p in reversed(processes):
            if p.poll() is None:
                p.terminate()
                try: p.wait(timeout=5)
                except subprocess.TimeoutExpired: p.kill(); p.wait()
        for log in logs: log.close()
        log = '\n'.join(p.read_text() for p in out.glob('gust*.log'))
        warnings = [line for line in log.splitlines() if any(w in line for w in ['ReferenceError:', 'TypeError:', 'Binding loop', 'Cannot assign', 'is not a function', 'Unable to assign'])]
        (out/'qml-warnings.txt').write_text('\n'.join(warnings))
        if warnings: result = 1
        (out/'receipt.json').write_text(json.dumps({'exit':result, 'checks':checks, 'binary_sha256':hashlib.sha256(binary.read_bytes()).hexdigest(),
            'scope':'real Rust/CXX/QML app in isolated labwc; Tailscale and devices are stand-ins; no live policy or physical iOS qualification'},indent=2)+'\n')
    return result


if __name__ == '__main__':
    sys.exit(main())
