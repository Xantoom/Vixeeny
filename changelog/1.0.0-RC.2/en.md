Second release candidate of Vixeeny 1.0. Please report anything that goes wrong: Settings → About → Report a problem.

## Fixes

- Screenshots of games in borderless full screen no longer show intermittent black bands at the top of the image.
- Full-screen games are less likely to stall or freeze at launch while the replay buffer starts: capture frames and GPU resources are released immediately, secondary window capture stays off for games, and GPU conversion is skipped when the encoder queue is full.
- Hardware encoder detection is serialised across processes and stops waiting after 15 seconds instead of 60.
- Games protected by an anti-cheat are no longer mistaken for closed, preventing the replay buffer and its startup notification from restarting in a loop.
- Switching between games or closing a game while the replay runs now moves cleanly to the next game in front.
