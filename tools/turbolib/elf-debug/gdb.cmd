set pagination off
set confirm off
set print address off
break fx::accumulate<int>
commands
  bt
  info args
  info locals
  list
  next
  next
  next
  info locals
  continue
end
break fx::Rect::area
commands
  bt
  ptype this
  print *this
  up
  info locals
  continue
end
break fx::with_lambda
commands
  bt
  info args
  step
  step
  bt
  info locals
  continue
end
break fx::mix
commands
  bt
  info args
  finish
  continue
end
break shared.h:23
break fx::checked
commands
  bt
  print fx::tls_depth
  continue
end
run
info breakpoints
info functions ^fx::
info types ^fx::
info scope fx::accumulate<int>
info scope fx::counter
info line fx::mix
list fx::with_lambda
print fx::counter()::calls
print sizeof(fx::Rect)
ptype fx::Point
