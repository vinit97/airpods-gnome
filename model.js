// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 AirPods for GNOME contributors
// Schema 1 adapter for the AirPods GNOME Rust backend. No Shell dependencies.
export const MODES = [
    ['Off', 'noise:off'],
    ['Noise Cancellation', 'noise:anc'],
    ['Transparency', 'noise:transparency'],
    ['Adaptive', 'noise:adaptive'],
];
export const EARS = [
    ['Pause when one is out', 'ear:one'],
    ['Pause when both are out', 'ear:both'],
    ['Never pause', 'ear:off'],
];
const COMMANDS = new Set([...MODES, ...EARS].map(([, command]) => command).concat('ca:on', 'ca:off'));

function battery(raw) {
    const available = raw?.available === true;
    return {
        level: available && Number.isInteger(raw.level) && raw.level >= 0 && raw.level <= 100
            ? raw.level : null,
        charging: available && raw.charging === true,
        inEar: available && raw.in_ear === true,
    };
}

export function parseStatus(text) {
    let raw;
    try {
        raw = JSON.parse(text);
    } catch {
        throw new Error('Could not read AirPods status');
    }
    if (!raw || raw.schema_version !== 1)
        throw new Error('Unsupported AirPods status format; update the extension and service');

    // Missing capabilities are unknown: do not offer controls the device may lack.
    const modes = raw.supports_noise_control === true
        ? [0, 2, 3, 1].filter(n => (n !== 0 || raw.supports_noise_off === true)
            && (n !== 3 || raw.supports_adaptive === true)) : [];
    return {
        connected: raw.connected === true,
        name: typeof raw.device_name === 'string' && raw.device_name ? raw.device_name : 'AirPods',
        model: typeof raw.model_name === 'string' ? raw.model_name : '',
        iconVariant: raw.is_headset === true ? 'max' : raw.is_pro_series === true ? 'pro' : 'buds',
        batteries: raw.is_headset === true
            ? [['Headphones', battery(raw.headset)]]
            : [['Left', battery(raw.left)], ['Right', battery(raw.right)], ['Case', battery(raw.case)]],
        modes,
        mode: raw.noise_mode,
        conversationSupported: raw.supports_conversational_awareness === true,
        conversation: raw.conversational_awareness === true,
        ear: raw.ear_detection_behavior,
        adaptive: Number.isInteger(raw.adaptive_noise_level) ? Math.max(0, Math.min(100, raw.adaptive_noise_level)) : 50,
    };
}

export function batteryDetail(name, battery) {
    if (battery.level === null)
        return name === 'Case' ? 'Waiting for case data' : 'Unavailable';
    return battery.charging ? 'Charging' : battery.inEar ? 'In ear' : '';
}

export function panelText(status) {
    if (!status) return 'AirPods';
    const levels = status.batteries.filter(([name, b]) => name !== 'Case' && b.level !== null).map(([, b]) => b.level);
    return levels.length ? `${Math.min(...levels)}%` : 'AirPods';
}

export function validCommand(verb) {
    if (typeof verb !== 'string') return false;
    if (COMMANDS.has(verb)) return true;
    if (!verb.startsWith('adaptive:')) return false;
    const level = Number(verb.slice('adaptive:'.length));
    return Number.isInteger(level) && level >= 0 && level <= 100 && verb === `adaptive:${level}`;
}

export function cycleMode(modes, current, direction) {
    if (!modes.length || ![1, -1].includes(direction)) return null;
    const index = modes.indexOf(current);
    if (index < 0) return direction > 0 ? modes[0] : modes[modes.length - 1];
    return modes[(index + direction + modes.length) % modes.length];
}
