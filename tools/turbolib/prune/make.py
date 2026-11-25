"""Sources for the type-pruning test: 200 structures, each used only by its own
function, so that /OPT:REF discards every function but `used`, and a /Yc
header holding all 200 whose one /Yu user refers to S0 alone."""
N = 200
h = ["#pragma once"]
for i in range(N):
    h.append(f"struct S{i} {{ int a{i}; double b{i}; S{i}* next; }};")
    h.append(f"inline int use{i}(S{i}* p) {{ S{i} local{{}}; local.next = p; return local.a{i} + (int)p->b{i}; }}")
open("big.h", "w").write("\n".join(h) + "\n")
a = ['#include "big.h"']
for i in range(N):
    a.append(f"int unused{i}(S{i}* p) {{ S{i} s{{}}; s.next = p; return s.a{i}; }}")
a.append("int used(S0* p) { S0 s{}; s.next = p; return s.a0; }")
open("a.cpp", "w").write("\n".join(a) + "\n")
open("main.cpp", "w").write("struct S0;\nint used(S0*);\nint main(){ return used(nullptr) & 0; }\n")
open("pch.cpp", "w").write('#include "big.h"\n')
open("user.cpp", "w").write('#include "big.h"\nint user0(S0* p) { S0 s{}; s.next = p; return s.a0; }\n')
open("main2.cpp", "w").write("struct S0;\nint user0(S0*);\nint main(){ return user0(nullptr) & 0; }\n")
