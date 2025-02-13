import { NotifiedElement, Placeholder, Types, StateNotifier } from "./framework.js";

function generateUUID() {
  let dt = new Date().getTime();
  const uuid = 'xxxxxxxx-xxxx-4xxx-yxxx-xxxxxxxxxxxx'.replace(/[xy]/g, function (c) {
    const r = (dt + Math.random() * 16) % 16 | 0;
    dt = Math.floor(dt / 16);
    const v = c == 'x' ? r : (r & 0x3 | 0x8);
    return v.toString(16).toUpperCase();
  });
  return uuid;
}

const user = {
  name: randomName(),
  avatar: `https://avatars.dicebear.com/api/avataaars/${randomName()}.svg`,
  uuid: generateUUID(),
}


const userStateNotifier = new StateNotifier(user);

const userWidget = new NotifiedElement({
  // TODO: Provide a way to pass the mount point and options
  // that is not error prone
  // mountPoint: document.querySelector("body"),
  // mountOptions: {
  //   templateExists: true,
  // },
  stateNotifier: userStateNotifier,
  templateFunction: function (_) {
    const user = this.stateNotifier.state
    return {
      root: "main.l-content div.user.you",
      '.avatar img': new Placeholder(Types.ATTR,
        {
          src: user.avatar,
          alt: user.name,
          title: `peer id: ${user.uuid}`
        }),
      '.user-info .user-ip #user-label': user.name,
    }
  }
});

userWidget.render();

// helpers
function randomName() {
  const adjectives = [
    "autumn", "hidden", "bitter", "misty", "silent", "empty", "dry", "dark",
    "summer", "icy", "delicate", "quiet", "white", "cool", "spring", "winter",
    "patient", "twilight", "dawn", "crimson", "wispy", "weathered", "blue",
  ]
  const nouns = [
    "waterfall", "river", "breeze", "moon", "rain", "wind", "sea", "morning",
    "snow", "lake", "sunset", "pine", "shadow", "leaf", "dawn", "glitter",
    "forest", "hill", "cloud", "meadow", "sun", "glade", "bird", "brook",
    "butterfly", "bush", "dew", "dust", "field", "fire", "flower", "firefly",
    "feather", "grass", "haze", "mountain", "night", "pond", "darkness",
    "snowflake", "silence", "sound", "sky", "shape", "surf", "thunder",
    "violet", "water", "wildflower", "wave", "water", "resonance", "sun",
    "wood", "dream", "cherry", "tree", "fog", "frost", "voice", "paper",
    "frog", "smoke", "star",
  ]
  return (
    adjectives[Math.floor(Math.random() * adjectives.length)] +
    "_" +
    nouns[Math.floor(Math.random() * nouns.length)]
  )
}
