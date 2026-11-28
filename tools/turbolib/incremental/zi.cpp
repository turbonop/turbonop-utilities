// A library object built with /Zi: its types are in a PDB of its own
// (a type server), not in the object.
#include "shared.h"

int zi_value() { return use_shared(4) - 1; }
