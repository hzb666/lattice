/** Errors shared by the signed theme and plugin registry. */
import { t } from "@lingui/core/macro";
import type { HostErrorEntry } from "./host-error-text";

/* eslint-disable lingui/no-unlocalized-strings -- host messages are matched as data; each translation is the t`` beside it */

export const REGISTRY_HOST_ERRORS: readonly HostErrorEntry[] = [
  ["registry/verify.rs", "The registry's {file} has no valid signature.", ([file]) => t`The registry's ${file} has no valid signature.`],
  ["registry/verify.rs", "The registry's {file} is signed for another file or serial.", ([file]) => t`The registry's ${file} is signed for another file or serial.`],
  ["registry.rs", "The registry's list is in a newer format. Update Lattice to use it.", () => t`The registry's list is in a newer format. Update Lattice to use it.`],
  ["registry.rs", "The registry is updating its list. Try again in a few minutes.", () => t`The registry is updating its list. Try again in a few minutes.`],
  ["registry.rs", "The registry sent an older list (serial {} after {}), so Lattice kept the one it has.", ([sent, kept]) => t`The registry sent an older list (serial ${sent} after ${kept}), so Lattice kept the one it has.`],
  ["registry.rs", "The registry could not be reached: {err}", ([error]) => t`The registry could not be reached: ${error}`],
  ["registry.rs", "The registry's {file} could not be read: {err}", ([file, error]) => t`The registry's ${file} could not be read: ${error}`],
  ["registry.rs", "Load the theme list before installing a theme from it.", () => t`Load the theme list before installing a theme from it.`],
  ["registry.rs", "\"{id}\" is not in the theme list.", ([id]) => t`"${id}" is not in the theme list.`],
  ["registry.rs", "{} {} is blocked: {}", ([name, version, reason]) => t`${name} ${version} is blocked: ${reason}`],
  ["registry.rs", "The theme download failed: {err}", ([error]) => t`The theme download failed: ${error}`],
  ["registry.rs", "The downloaded theme is not the size the theme list gives.", () => t`The downloaded theme is not the size the theme list gives.`],
  ["registry.rs", "The downloaded theme does not match the theme list's checksum.", () => t`The downloaded theme does not match the theme list's checksum.`],
  ["registry.rs", "The downloaded theme is not \"{id}\" {}.", ([id, version]) => t`The downloaded theme is not "${id}" ${version}.`],
  ["registry.rs", "{url} is not an https address.", ([url]) => t`${url} is not an https address.`],
  ["registry.rs", "{url} is larger than expected.", ([url]) => t`${url} is larger than expected.`],
  ["registry.rs", "Could not save the theme list: {err}", ([error]) => t`Could not save the theme list: ${error}`],
];
