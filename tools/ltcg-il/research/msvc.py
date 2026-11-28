"""Runs an MSVC tool with the environment vcvarsall gave (env64.txt / env86.txt).

usage: python msvc.py [-x86] tool args...   (run in the current directory)
"""
import os, subprocess, sys

here = os.path.dirname(os.path.abspath(__file__))
args = sys.argv[1:]
envfile = 'env64.txt'
if args and args[0] == '-x86':
    envfile = 'env86.txt'
    args = args[1:]
env = {}
for line in open(os.path.join(here, envfile), encoding='mbcs', errors='replace'):
    k, _, v = line.rstrip('\r\n').partition('=')
    if k:
        env[k] = v
env['VSCMD_SKIP_SENDTELEMETRY'] = '1'
import shutil
path = next(v for k, v in env.items() if k.lower() == 'path')
args[0] = shutil.which(args[0], path=path) or args[0]
r = subprocess.run(args, env=env, stdin=subprocess.DEVNULL)
sys.exit(r.returncode)
