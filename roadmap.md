# roadmap

rift is still in beta despite being quite feature rich and *relatively* bug free. this is a non exhaustive list of things that need to be done pre 1.0:

- configuration. currently rift can only be configured via a toml file. this is not ideal and a gui and/or cli is needed to make configuration easier
- scriptable layouts. due to the fact that rift exposes a mach port, performant communication with third party programs should be possible. this would allow for BYO layouts and total customization of how rift behaves
- testing. rift is somewhat tested as of now but i would like for there to be a comprehensive test suite that ensures that nothing breaks(there is a framework to synthesize events and state which will be used to create this test suite)
- sticky windows
- scratchpad windows/workspace
- docked windows that sit on the outside of the screen (maybe sticks out by 10px) and when hovered slides in to view
- more layout styles (ie monocle, grid, etc)
- more animations (ie window minimize/maximize, workspace switch, etc)
- more configuration options (ie gaps, padding, etc)
- better multi monitor support (ie different gaps per monitor, etc)
- ensure total isolation between macos spaces. rift currently tries to do this but there are still some holes that need to be plugged
