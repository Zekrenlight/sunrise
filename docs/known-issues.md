# Known issues

Every game release lists the bullets below under "Known issues" (tools/release_train.py). Keep them short and
written for players; remove a bullet in the pull request that fixes it.

- Launchers 1.0.0 and 1.0.1 sign in through the Starling launcher's app; launcher 1.1.0 and later use Sunrise's own
  Microsoft app, approved by Mojang. Updating from 1.0.0 or 1.0.1 asks you to sign in once more.
- Directional Audio doesn't sound like Minecraft's yet: Sunrise models a simple round head instead of using OpenAL
  Soft's HRTF. Stereo sounds aren't processed by it, a sound at your own position is heard as straight ahead, and on
  surround speakers both ears play through the first two channels.
