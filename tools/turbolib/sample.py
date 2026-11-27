"""A sampling CPU profiler for a Windows program, without administrator
rights: it runs the program, and every interval suspends each of its
threads, walks its stack (dbghelp's StackWalk64, from the unwind tables),
and resumes it. Then it names each frame from the PDB, inline frames
included, and prints the functions by self and by total samples.

    python sample.py [--interval MS] [--top N] [--cwd DIR] [--env K=V]...
                     [--json FILE] -- <program> <arguments>...

Only the program's own module is named (other modules' frames are counted
as their module). Suspending threads perturbs what it measures a little:
compare runs made the same way, not to unsampled times.
"""
import argparse
import collections
import ctypes
import json
import os
import subprocess
import sys
import threading
import time
from ctypes import wintypes

k32 = ctypes.WinDLL("kernel32", use_last_error=True)
dbghelp = ctypes.WinDLL("dbghelp", use_last_error=True)

TH32CS_SNAPTHREAD = 0x4
THREAD_ALL = 0x0002 | 0x0008 | 0x0040 | 0x0010  # suspend/resume, get context, query
CONTEXT_AMD64 = 0x100000
CONTEXT_FULL = CONTEXT_AMD64 | 0x1 | 0x2 | 0x8


class THREADENTRY32(ctypes.Structure):
    _fields_ = [("dwSize", wintypes.DWORD), ("cntUsage", wintypes.DWORD),
                ("th32ThreadID", wintypes.DWORD), ("th32OwnerProcessID", wintypes.DWORD),
                ("tpBasePri", wintypes.LONG), ("tpDeltaPri", wintypes.LONG),
                ("dwFlags", wintypes.DWORD)]


class M128(ctypes.Structure):
    _fields_ = [("lo", ctypes.c_ulonglong), ("hi", ctypes.c_longlong)]


class CONTEXT(ctypes.Structure):
    _pack_ = 16
    _fields_ = ([(n, ctypes.c_ulonglong) for n in ("P1", "P2", "P3", "P4", "P5", "P6")]
                + [("ContextFlags", wintypes.DWORD), ("MxCsr", wintypes.DWORD)]
                + [(n, wintypes.WORD) for n in ("SegCs", "SegDs", "SegEs", "SegFs", "SegGs", "SegSs")]
                + [("EFlags", wintypes.DWORD)]
                + [(n, ctypes.c_ulonglong) for n in ("Dr0", "Dr1", "Dr2", "Dr3", "Dr6", "Dr7")]
                + [(n, ctypes.c_ulonglong) for n in (
                    "Rax", "Rcx", "Rdx", "Rbx", "Rsp", "Rbp", "Rsi", "Rdi", "R8", "R9", "R10",
                    "R11", "R12", "R13", "R14", "R15", "Rip")]
                + [("FltSave", ctypes.c_ubyte * 512), ("VectorRegister", M128 * 26),
                   ("VectorControl", ctypes.c_ulonglong), ("DebugControl", ctypes.c_ulonglong),
                   ("LastBranchToRip", ctypes.c_ulonglong), ("LastBranchFromRip", ctypes.c_ulonglong),
                   ("LastExceptionToRip", ctypes.c_ulonglong),
                   ("LastExceptionFromRip", ctypes.c_ulonglong)])


class ADDRESS64(ctypes.Structure):
    _fields_ = [("Offset", ctypes.c_ulonglong), ("Segment", wintypes.WORD), ("Mode", ctypes.c_int)]


class KDHELP64(ctypes.Structure):
    _fields_ = [(n, ctypes.c_ulonglong) for n in (
        "Thread", "ThCallbackStack", "ThCallbackBStore", "NextCallback", "FramePointer",
        "KiCallUserMode", "KeUserCallbackDispatcher", "SystemRangeStart",
        "KiUserExceptionDispatcher", "StackBase", "StackLimit", "BuildVersion",
        "RetpolineStubFunctionTableSize", "RetpolineStubFunctionTable",
        "RetpolineStubOffset", "RetpolineStubSize", "Reserved0", "Reserved1")]


class STACKFRAME64(ctypes.Structure):
    _fields_ = [("AddrPC", ADDRESS64), ("AddrReturn", ADDRESS64), ("AddrFrame", ADDRESS64),
                ("AddrStack", ADDRESS64), ("AddrBStore", ADDRESS64),
                ("FuncTableEntry", ctypes.c_void_p), ("Params", ctypes.c_ulonglong * 4),
                ("Far", wintypes.BOOL), ("Virtual", wintypes.BOOL),
                ("Reserved", ctypes.c_ulonglong * 3), ("KdHelp", KDHELP64)]


class SYMBOL_INFO(ctypes.Structure):
    _fields_ = [("SizeOfStruct", wintypes.ULONG), ("TypeIndex", wintypes.ULONG),
                ("Reserved", ctypes.c_ulonglong * 2), ("Index", wintypes.ULONG),
                ("Size", wintypes.ULONG), ("ModBase", ctypes.c_ulonglong),
                ("Flags", wintypes.ULONG), ("Value", ctypes.c_ulonglong),
                ("Address", ctypes.c_ulonglong), ("Register", wintypes.ULONG),
                ("Scope", wintypes.ULONG), ("Tag", wintypes.ULONG),
                ("NameLen", wintypes.ULONG), ("MaxNameLen", wintypes.ULONG),
                ("Name", ctypes.c_char * 2000)]


class MODULEENTRY32W(ctypes.Structure):
    _fields_ = [("dwSize", wintypes.DWORD), ("th32ModuleID", wintypes.DWORD),
                ("th32ProcessID", wintypes.DWORD), ("GlblcntUsage", wintypes.DWORD),
                ("ProccntUsage", wintypes.DWORD), ("modBaseAddr", ctypes.c_void_p),
                ("modBaseSize", wintypes.DWORD), ("hModule", wintypes.HMODULE),
                ("szModule", ctypes.c_wchar * 256), ("szExePath", ctypes.c_wchar * 260)]


k32.CreateToolhelp32Snapshot.restype = wintypes.HANDLE
k32.OpenThread.restype = wintypes.HANDLE
k32.GetThreadContext.argtypes = [wintypes.HANDLE, ctypes.c_void_p]
k32.SuspendThread.argtypes = [wintypes.HANDLE]
k32.ResumeThread.argtypes = [wintypes.HANDLE]
k32.CloseHandle.argtypes = [wintypes.HANDLE]
k32.QueryThreadCycleTime.argtypes = [wintypes.HANDLE, ctypes.POINTER(ctypes.c_ulonglong)]
k32.Thread32First.argtypes = [wintypes.HANDLE, ctypes.POINTER(THREADENTRY32)]
k32.Thread32Next.argtypes = [wintypes.HANDLE, ctypes.POINTER(THREADENTRY32)]
k32.Module32FirstW.argtypes = [wintypes.HANDLE, ctypes.POINTER(MODULEENTRY32W)]
k32.Module32NextW.argtypes = [wintypes.HANDLE, ctypes.POINTER(MODULEENTRY32W)]
dbghelp.SymInitializeW.argtypes = [wintypes.HANDLE, wintypes.LPCWSTR, wintypes.BOOL]
dbghelp.SymFunctionTableAccess64.restype = ctypes.c_void_p
dbghelp.SymFunctionTableAccess64.argtypes = [wintypes.HANDLE, ctypes.c_ulonglong]
dbghelp.SymGetModuleBase64.restype = ctypes.c_ulonglong
dbghelp.SymGetModuleBase64.argtypes = [wintypes.HANDLE, ctypes.c_ulonglong]
FTA = ctypes.WINFUNCTYPE(ctypes.c_void_p, wintypes.HANDLE, ctypes.c_ulonglong)
GMB = ctypes.WINFUNCTYPE(ctypes.c_ulonglong, wintypes.HANDLE, ctypes.c_ulonglong)
dbghelp.StackWalk64.argtypes = [wintypes.DWORD, wintypes.HANDLE, wintypes.HANDLE,
                                ctypes.POINTER(STACKFRAME64), ctypes.c_void_p, ctypes.c_void_p,
                                FTA, GMB, ctypes.c_void_p]
dbghelp.SymLoadModuleExW.restype = ctypes.c_ulonglong
dbghelp.SymLoadModuleExW.argtypes = [wintypes.HANDLE, wintypes.HANDLE, wintypes.LPCWSTR,
                                     wintypes.LPCWSTR, ctypes.c_ulonglong, wintypes.DWORD,
                                     ctypes.c_void_p, wintypes.DWORD]
dbghelp.SymFromAddr.argtypes = [wintypes.HANDLE, ctypes.c_ulonglong,
                                ctypes.POINTER(ctypes.c_ulonglong), ctypes.POINTER(SYMBOL_INFO)]
dbghelp.SymAddrIncludeInlineTrace.restype = wintypes.DWORD
dbghelp.SymAddrIncludeInlineTrace.argtypes = [wintypes.HANDLE, ctypes.c_ulonglong]
dbghelp.SymQueryInlineTrace.argtypes = [wintypes.HANDLE, ctypes.c_ulonglong, wintypes.DWORD,
                                        ctypes.c_ulonglong, ctypes.c_ulonglong,
                                        ctypes.POINTER(wintypes.DWORD),
                                        ctypes.POINTER(wintypes.DWORD)]
dbghelp.SymFromInlineContext.argtypes = [wintypes.HANDLE, ctypes.c_ulonglong, wintypes.DWORD,
                                         ctypes.POINTER(ctypes.c_ulonglong),
                                         ctypes.POINTER(SYMBOL_INFO)]

FTA_CB = FTA(dbghelp.SymFunctionTableAccess64)
GMB_CB = GMB(dbghelp.SymGetModuleBase64)


def threads_of(pid):
    snap = k32.CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0)
    out = []
    e = THREADENTRY32()
    e.dwSize = ctypes.sizeof(e)
    ok = k32.Thread32First(snap, ctypes.byref(e))
    while ok:
        if e.th32OwnerProcessID == pid:
            out.append(e.th32ThreadID)
        ok = k32.Thread32Next(snap, ctypes.byref(e))
    k32.CloseHandle(snap)
    return out


def modules_of(pid):
    snap = k32.CreateToolhelp32Snapshot(0x8 | 0x10, pid)
    out = []
    e = MODULEENTRY32W()
    e.dwSize = ctypes.sizeof(e)
    ok = k32.Module32FirstW(snap, ctypes.byref(e))
    while ok:
        out.append((e.modBaseAddr or 0, e.modBaseSize, e.szModule, e.szExePath))
        ok = k32.Module32NextW(snap, ctypes.byref(e))
    k32.CloseHandle(snap)
    return out


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--interval", type=float, default=1.0)
    ap.add_argument("--top", type=int, default=40)
    ap.add_argument("--depth", type=int, default=96,
                    help="frames walked (1: the leaf alone, much faster)")
    ap.add_argument("--cwd")
    ap.add_argument("--env", action="append", default=[])
    ap.add_argument("--json")
    ap.add_argument("--timeout", type=float, default=600)
    ap.add_argument("command", nargs=argparse.REMAINDER)
    a = ap.parse_args()
    cmd = a.command[1:] if a.command and a.command[0] == "--" else a.command
    env = dict(os.environ)
    for kv in a.env:
        k, _, v = kv.partition("=")
        env[k] = v
    p = subprocess.Popen(cmd, cwd=a.cwd, env=env, stdout=subprocess.PIPE,
                         stderr=subprocess.STDOUT)
    hp = int(p._handle)
    time.sleep(0.02)
    dbghelp.SymInitializeW(hp, None, True)
    output = []
    reader = threading.Thread(target=lambda: output.append(p.stdout.read()), daemon=True)
    reader.start()
    stacks = collections.Counter()
    handles = {}
    ctx_buf = ctypes.create_string_buffer(ctypes.sizeof(CONTEXT) + 16)
    addr = ctypes.addressof(ctx_buf)
    ctx = CONTEXT.from_address((addr + 15) & ~15)
    modules = []
    t0 = time.perf_counter()
    last_snap = 0
    samples = 0
    idle = 0
    last_cycles = {}
    refreshed = 0
    while p.poll() is None and time.perf_counter() - t0 < a.timeout:
        now = time.perf_counter()
        if now - last_snap > 0.02:
            last_snap = now
            live = set(threads_of(p.pid))
            for tid in list(handles):
                if tid not in live:
                    k32.CloseHandle(handles.pop(tid))
            for tid in live:
                if tid not in handles:
                    h = k32.OpenThread(THREAD_ALL, False, tid)
                    if h:
                        handles[tid] = h
            if now - refreshed > 0.2:
                refreshed = now
                dbghelp.SymRefreshModuleList(hp)
                if not modules:
                    modules = modules_of(p.pid)
        for tid, h in list(handles.items()):
            # A thread that ran no cycles since the last sample is waiting:
            # not counted.
            cycles = ctypes.c_ulonglong()
            k32.QueryThreadCycleTime(h, ctypes.byref(cycles))
            if cycles.value - last_cycles.get(tid, 0) < 20000:
                last_cycles[tid] = cycles.value
                idle += 1
                continue
            last_cycles[tid] = cycles.value
            if k32.SuspendThread(h) == 0xFFFFFFFF:
                continue
            try:
                ctx.ContextFlags = CONTEXT_FULL
                if not k32.GetThreadContext(h, ctypes.byref(ctx)):
                    continue
                f = STACKFRAME64()
                f.AddrPC.Offset, f.AddrPC.Mode = ctx.Rip, 3
                f.AddrFrame.Offset, f.AddrFrame.Mode = ctx.Rbp, 3
                f.AddrStack.Offset, f.AddrStack.Mode = ctx.Rsp, 3
                frames = [ctx.Rip] if a.depth == 1 else []
                while a.depth > 1 and len(frames) < a.depth and dbghelp.StackWalk64(
                        0x8664, hp, h, ctypes.byref(f), ctypes.byref(ctx), None, FTA_CB,
                        GMB_CB, None):
                    if f.AddrPC.Offset == 0:
                        break
                    frames.append(f.AddrPC.Offset)
            finally:
                k32.ResumeThread(h)
            if frames:
                # Idle threads wait in the kernel: their leaf is ntdll's.
                stacks[tuple(frames)] += 1
                samples += 1
        time.sleep(a.interval / 1000)
    if p.poll() is None:
        p.kill()
    p.wait(timeout=60)
    reader.join(timeout=10)
    wall = time.perf_counter() - t0
    if not modules:
        sys.exit("the program ended before its modules were read")
    for h in handles.values():
        k32.CloseHandle(h)
    dbghelp.SymCleanup(hp)

    # Name the frames offline, from the program's file and PDB.
    exe = modules[0]
    me = ctypes.c_void_p(12345)
    dbghelp.SymSetOptions(0x2 | 0x4 | 0x10)  # undecorate, deferred loads, line numbers
    dbghelp.SymInitializeW(me, os.path.dirname(exe[3]), False)
    base = dbghelp.SymLoadModuleExW(me, None, exe[3], None, exe[0], exe[1], None, 0)
    if not base:
        print("SymLoadModuleEx failed", ctypes.get_last_error(), file=sys.stderr)
    # Other modules by their exports (no symbol server is asked).
    for b0, size, mod, path in modules[1:]:
        dbghelp.SymLoadModuleExW(me, None, path, None, b0, size, None, 0)
    names = {}

    def frame_names(pc):
        """Outermost first: the function, then the inlined ones inside it."""
        if pc in names:
            return names[pc]
        if not (exe[0] <= pc < exe[0] + exe[1]):
            for b, size, mod, _ in modules:
                if b <= pc < b + size:
                    sym = SYMBOL_INFO()
                    sym.SizeOfStruct = 88
                    sym.MaxNameLen = 2000
                    disp = ctypes.c_ulonglong()
                    if dbghelp.SymFromAddr(me, pc, ctypes.byref(disp), ctypes.byref(sym)):
                        names[pc] = [f"{mod}!{sym.Name.decode(errors='replace')}"]
                    else:
                        names[pc] = [mod]
                    return names[pc]
            names[pc] = ["?"]
            return names[pc]
        sym = SYMBOL_INFO()
        sym.SizeOfStruct = 88
        sym.MaxNameLen = 2000
        disp = ctypes.c_ulonglong()
        out = []
        if dbghelp.SymFromAddr(me, pc, ctypes.byref(disp), ctypes.byref(sym)):
            out.append(sym.Name.decode(errors="replace"))
        else:
            out.append(f"{exe[2]}+{pc - exe[0]:#x}")
        n = dbghelp.SymAddrIncludeInlineTrace(me, pc)
        if n:
            ictx, idx = wintypes.DWORD(), wintypes.DWORD()
            if dbghelp.SymQueryInlineTrace(me, pc, 0, pc, pc, ctypes.byref(ictx), ctypes.byref(idx)):
                inl = []
                for i in range(n):
                    s2 = SYMBOL_INFO()
                    s2.SizeOfStruct = 88
                    s2.MaxNameLen = 2000
                    if dbghelp.SymFromInlineContext(me, pc, ictx.value + i, ctypes.byref(disp),
                                                    ctypes.byref(s2)):
                        inl.append(s2.Name.decode(errors="replace"))
                # The trace is innermost first.
                out.extend(reversed(inl))
        names[pc] = out
        return out

    self_count = collections.Counter()
    total_count = collections.Counter()
    # Threads waiting in the kernel (the thread pool's idle workers wake
    # now and then) aren't working either.
    for stack, n in list(stacks.items()):
        leaf = frame_names(stack[0])[-1]
        if "!" in leaf and any(w in leaf for w in ("Wait", "Delay", "RemoveIoCompletion")):
            idle += n
            samples -= n
            del stacks[stack]
    for stack, n in stacks.items():
        # The return addresses point after their calls: name call sites
        # by the byte before.
        chain = []
        for i, pc in enumerate(reversed(stack)):
            chain.extend(frame_names(pc if i == len(stack) - 1 else pc - 1))
        if chain:
            self_count[chain[-1]] += n
        for name in set(chain):
            total_count[name] += n
    print("".join(o.decode(errors="replace") for o in output if o)[-3000:])
    print(f"{samples} samples working, {idle} waiting, in {wall:.2f} s ({len(stacks)} stacks)")
    samples = max(samples, 1)
    print(f"{'self':>7} {'total':>7}  function")
    for name, n in self_count.most_common(a.top):
        print(f"{100 * n / samples:6.1f}% {100 * total_count[name] / samples:6.1f}%  {name[:150]}")
    print("by total:")
    for name, n in total_count.most_common(a.top):
        print(f"{100 * self_count[name] / samples:6.1f}% {100 * n / samples:6.1f}%  {name[:150]}")
    if a.json:
        with open(a.json, "w") as f:
            json.dump({"samples": samples, "stacks": [
                {"n": n, "frames": [x for pc in reversed(s) for x in frame_names(pc)]}
                for s, n in stacks.items()]}, f)


if __name__ == "__main__":
    main()
