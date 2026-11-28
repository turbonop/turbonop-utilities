// A library object (a precompiled-header user). VERSION changes its own
// function, to rebuild the library with one member changed.
#include "shared.h"

#ifndef VERSION
#define VERSION 1
#endif

// (Declared without __stdcall: it takes no arguments, so the conventions
// agree, and the x86 name is then the one kernel32.def gives.)
extern "C" __declspec(dllimport) unsigned long GetCurrentProcessId();

#if VERSION > 1
// Version 2 also uses an instantiation c.cpp has, so the library's
// synthesized members change too.
int a_value() { return use_shared(1) * VERSION + static_cast<int>(twice(40LL)); }
#else
int a_value() { return use_shared(1) * VERSION + twice(VERSION); }
#endif

// Through the import library archived with the objects.
int a_pid_ok() { return GetCurrentProcessId() != 0; }
