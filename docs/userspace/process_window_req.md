kiro-cli --resume-id 7d730480-2055-4189-9ec9-e06d6657d1e3





Moving theophe to a userspace program, ive noticed that the graphics backend is
  entirely on the kernel now, userspace program now doesnt have access to
  WindowBackBuffer functions. Same goes for the whole renderer small custom graphics
  API. What would be the best way to give programs access to my graphics API? Theophe is
  kind of a special case, it uses embedded-graphics which needs something that
  implements DrawTarget, so should i just to implement this WindowBackBuffer structure
  as a library usable for programs? For the 2D and 3D software rendering, should i go
  with command building, batching approach and then have kernel handle that with its
  rendering cores, or also move it all to userspace?