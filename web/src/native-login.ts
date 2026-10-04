import { nativeCarbonLink } from './protocol';

const callback = location.href;
history.replaceState(null, '', location.pathname);
let link = nativeCarbonLink(callback);
const message = document.getElementById('message')!;
const button = document.getElementById('open-ring') as HTMLButtonElement;
if (link) {
  message.textContent = 'Your sign-in is ready. Open Ring to finish connecting on this device.';
  button.hidden = false;
  const open = () => { if (link) location.replace(link); };
  button.addEventListener('click', open);
  setTimeout(() => {
    link = null; button.hidden = true;
    message.textContent = 'This sign-in link expired. Return to Ring and choose Continue as Carbon again.';
  }, 120000);
  open();
} else {
  message.textContent = 'This sign-in link is incomplete. Return to Ring and choose Continue as Carbon again.';
}
