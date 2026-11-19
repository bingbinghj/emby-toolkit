"""实际 worker 启动、宿主退出与进程组终止，不访问用户曲库。"""
import json
import os
from pathlib import Path
import signal
import subprocess
import sys
import time

import pytest

WORKER = Path(__file__).parents[1]/'worker.py'


def exited(pid):
    status = Path(f'/proc/{pid}/stat')
    return not status.exists() or status.read_text().split()[2] == 'Z'


def test_worker_json_protocol(tmp_path):
    request = {'action':'cache', 'body':{}, 'config':{}, 'cache_dir':str(tmp_path/'cache')}
    result = subprocess.run([sys.executable, '-B', str(WORKER)], input=json.dumps(request), text=True, capture_output=True, timeout=15)
    assert result.returncode == 0, result.stderr
    assert json.loads(result.stdout) == {'success':True, 'data':{'items':[]}}


@pytest.mark.skipif(not sys.platform.startswith('linux'), reason='Linux 宿主退出通知')
def test_worker_exits_when_host_dies(tmp_path):
    script = '''import os,subprocess,sys,time
process=subprocess.Popen([sys.executable,'-B',sys.argv[1]],stdin=subprocess.PIPE,start_new_session=True,env={**os.environ,'ETK_MUSIC_HOST_PID':str(os.getpid())})
print(process.pid,flush=True)
time.sleep(60)
'''
    host = subprocess.Popen([sys.executable,'-B','-c',script,str(WORKER)], stdout=subprocess.PIPE, text=True)
    pid = int(host.stdout.readline())
    try:
        # 等待 worker 读入请求；父进程持有 stdin，正常情况下它会一直等待。
        time.sleep(0.25)
        host.kill(); host.wait(timeout=5)
        deadline = time.monotonic()+5
        while not exited(pid) and time.monotonic()<deadline:
            time.sleep(0.02)
        assert exited(pid)
    finally:
        if host.poll() is None: host.kill(); host.wait()
        if not exited(pid): os.killpg(pid,signal.SIGKILL)

@pytest.mark.skipif(not sys.platform.startswith('linux'), reason='Linux 音乐进程组')
def test_terminating_worker_also_stops_fingerprint_process(tmp_path):
    import wave
    root, seed, binaries = tmp_path/'library', tmp_path/'seed', tmp_path/'bin'
    for directory in (root, seed, binaries): directory.mkdir()
    with wave.open(str(root/'01.wav'), 'wb') as audio:
        audio.setnchannels(2); audio.setsampwidth(2); audio.setframerate(44100); audio.writeframes(bytes(176400))
    ready = tmp_path/'fingerprint.pid'
    fpcalc = binaries/'fpcalc'
    fpcalc.write_text(f'#!{sys.executable}\nimport os,time\nfrom pathlib import Path\nPath({str(ready)!r}).write_text(str(os.getpid()))\ntime.sleep(60)\n')
    fpcalc.chmod(0o755)
    process = subprocess.Popen([sys.executable,'-B',str(WORKER)], stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True, start_new_session=True, env={**os.environ,'PATH':str(binaries)+os.pathsep+os.environ['PATH']})
    child = None
    try:
        process.stdin.write(json.dumps({'action':'preview','body':{'paths':['01.wav']},'cache_dir':str(tmp_path/'cache'),'config':{'music_library_path':str(root),'music_seed_path':str(seed)}})); process.stdin.close()
        deadline = time.monotonic()+10
        while not ready.exists() and process.poll() is None and time.monotonic()<deadline:
            time.sleep(0.02)
        assert ready.exists(), 'worker 未启动音频指纹进程'
        child = int(ready.read_text())
        process.terminate(); process.wait(timeout=5)
        deadline = time.monotonic()+3
        while not exited(child) and time.monotonic()<deadline: time.sleep(0.02)
        assert exited(child)
    finally:
        try: os.killpg(process.pid,signal.SIGKILL)
        except ProcessLookupError: pass
        process.wait(timeout=5)
