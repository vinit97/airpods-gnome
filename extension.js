// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Vinit Patel
import St from 'gi://St';
import Gio from 'gi://Gio';
import GLib from 'gi://GLib';
import Clutter from 'gi://Clutter';
import Atk from 'gi://Atk';
import Cairo from 'cairo';
import {Extension} from 'resource:///org/gnome/shell/extensions/extension.js';
import * as Main from 'resource:///org/gnome/shell/ui/main.js';
import * as PanelMenu from 'resource:///org/gnome/shell/ui/panelMenu.js';
import * as PopupMenu from 'resource:///org/gnome/shell/ui/popupMenu.js';
import {Slider} from 'resource:///org/gnome/shell/ui/slider.js';
import {Backend} from './backend.js';
import {MODES, EARS, panelText, batteryDetail, cycleMode} from './model.js';

const SELECTION_SETTLE_MS = 4000;
const SLIDER_DEBOUNCE_MS = 180;
const EAR_ORDER = [2, 0, 1];

function label(text, styleClass, expand = false) {
    return new St.Label({text, style_class: styleClass, x_expand: expand,
        y_align: Clutter.ActorAlign.CENTER,
        opacity: styleClass === 'airpods-subtitle' ? 180 : 255});
}

export default class AirPodsExtension extends Extension {
    enable() {
        this._enabled = true;
        this._syncing = false;
        this._lastStatus = null;
        this._pendingSelections = new Map();
        this._sliderTimeout = 0;
        this._sliderDragging = false;
        this._scrollAmount = 0;
        this._scrollAt = 0;
        try {
            this._buildMenu();
        } catch (error) {
            this.disable();
            throw error;
        }
        try {
            this._backend = new Backend(status => this._render(status),
                (message, kind, command, selection) => this._handleError(message, kind, command, selection));
        } catch (error) {
            console.error('AirPods backend initialization failed', error);
            this._handleError(error.message, 'status');
        }
    }

    _buildMenu() {
        this._button = new PanelMenu.Button(0.0, 'AirPods');
        this._button.menu.actor.add_style_class_name('airpods-menu');
        this._icons = Object.fromEntries(['buds', 'pro', 'max'].map(variant =>
            [variant, Gio.icon_new_for_string(`${this.path}/icons/airpods-${variant}-symbolic.svg`)]));
        this._batteryIcons = Object.fromEntries(['buds-left', 'buds-right', 'pro-left', 'pro-right', 'case'].map(name =>
            [name, Gio.icon_new_for_string(`${this.path}/icons/airpods-${name}-symbolic.svg`)]));
        this._batteryIcons.charging = Gio.icon_new_for_string(`${this.path}/icons/charging-symbolic.svg`);
        const icon = this._icons.buds;
        const panel = new St.BoxLayout({style_class: 'panel-status-menu-box'});
        this._panelIcon = new St.Icon({gicon: icon, style_class: 'system-status-icon'});
        panel.add_child(this._panelIcon);
        this._label = label('AirPods', 'airpods-panel-label');
        panel.add_child(this._label);
        this._button.add_child(panel);
        this._button._clickGesture.set_required_button(Clutter.BUTTON_PRIMARY);
        const cycleGesture = new Clutter.ClickGesture({required_button: Clutter.BUTTON_SECONDARY});
        cycleGesture.connect('recognize', () => this._cycleMode(1));
        this._button.add_action(cycleGesture);
        this._button.connect('scroll-event', (_actor, event) => this._onPanelScroll(event));

        const header = this._row('airpods-header');
        this._heroIcon = new St.Icon({gicon: icon, icon_size: 28, style_class: 'airpods-hero-icon'});
        header.add_child(this._heroIcon);
        this._title = label('AirPods', 'airpods-title', true);
        header.add_child(this._title);

        this._batteryGroup = this._row('airpods-batteries');
        const batteries = new St.BoxLayout({x_expand: true, style_class: 'airpods-battery-group'});
        batteries.layout_manager.homogeneous = true;
        this._batteries = ['Left', 'Right', 'Case'].map(name => this._batteryRing(name));
        for (const index of [0, 2, 1]) batteries.add_child(this._batteries[index].row);
        this._batteryGroup.add_child(batteries);

        this._listeningSeparator = this._separator();
        this._modeButtonsRow = this._row('airpods-modes-row');
        const modeButtons = new St.BoxLayout({x_expand: true, style_class: 'airpods-mode-buttons'});
        modeButtons.layout_manager.homogeneous = true;
        this._modeButtonsRow.add_child(modeButtons);
        this._modes = MODES.map(([name], index) => {
            const item = new St.Button({style_class: 'airpods-mode-button', can_focus: true,
                reactive: true, track_hover: true, x_expand: true,
                accessible_name: name, accessible_role: Atk.Role.RADIO_BUTTON});
            const content = new St.BoxLayout({orientation: Clutter.Orientation.VERTICAL,
                style_class: 'airpods-mode-button-content'});
            content.add_child(new St.Icon({icon_name: ['media-playback-stop-symbolic',
                'audio-volume-muted-symbolic', 'audio-volume-high-symbolic', 'view-refresh-symbolic'][index],
                icon_size: 18, x_align: Clutter.ActorAlign.CENTER}));
            const title = label(index === 1 ? 'Noise\nCancellation' : name, 'airpods-mode-button-label');
            title.x_align = Clutter.ActorAlign.CENTER;
            content.add_child(title);
            item.set_child(content);
            item.connect('clicked', () => this._selectMode(index));
            return item;
        });
        for (const index of [0, 2, 3, 1]) modeButtons.add_child(this._modes[index]);

        this._adaptive = this._row('airpods-adaptive-row');
        const adaptiveBox = new St.BoxLayout({orientation: Clutter.Orientation.VERTICAL,
            x_expand: true, style_class: 'airpods-adaptive-box'});
        const adaptiveTitle = new St.BoxLayout();
        adaptiveTitle.add_child(label('Adaptive noise level', 'airpods-subtitle', true));
        this._adaptiveValue = label('50%', 'airpods-subtitle');
        adaptiveTitle.add_child(this._adaptiveValue);
        adaptiveBox.add_child(adaptiveTitle);
        this._slider = new Slider(0.5);
        this._slider.accessible_name = 'Adaptive noise level';
        this._slider.connect('drag-begin', () => { this._sliderDragging = true; });
        this._slider.connect('drag-end', () => {
            if (!this._enabled) return;
            this._sliderDragging = false;
            const pending = this._pendingSelections.get('adaptive');
            if (pending) this._holdSelection('adaptive', pending.value, pending.command, pending);
            else this._render(this._lastStatus);
        });
        this._slider.connect('notify::value', () => {
            if (this._syncing || !this._adaptive.visible) return;
            const value = Math.round(this._slider.value * 100);
            this._adaptiveValue.text = `${value}%`;
            if (value === this._displayedValue('adaptive')) return;
            const pending = this._holdSelection('adaptive', value, `adaptive:${value}`);
            if (this._sliderTimeout) GLib.Source.remove(this._sliderTimeout);
            this._sliderTimeout = GLib.timeout_add(GLib.PRIORITY_DEFAULT, SLIDER_DEBOUNCE_MS, () => {
                this._sliderTimeout = 0;
                this._request(pending.command, pending);
                return GLib.SOURCE_REMOVE;
            });
        });
        adaptiveBox.add_child(this._slider);
        this._adaptive.add_child(adaptiveBox);

        this._optionsSeparator = this._separator();
        this._conversation = this._row('airpods-feature-row');
        this._conversation.add_child(label('Conversation Awareness', 'airpods-feature-title', true));
        this._conversationToggle = new St.Button({style_class: 'airpods-switch-button',
            can_focus: true, reactive: true, track_hover: true,
            accessible_name: 'Conversation Awareness', accessible_role: Atk.Role.CHECK_BOX});
        this._conversationSwitch = new PopupMenu.Switch(false);
        // The button owns pointer/keyboard activation; rendering the switch never sends commands.
        this._conversationSwitch.reactive = false;
        this._conversationToggle.set_child(this._conversationSwitch);
        this._conversationToggle.connect('clicked', () => {
            this._selectConversation(!this._displayedValue('conversation'));
        });
        this._conversation.add_child(this._conversationToggle);

        this._ears = this._row('airpods-feature-row');
        this._ears.add_child(label('Ear Detection', 'airpods-feature-title', true));
        const earChoices = new St.BoxLayout({style_class: 'airpods-ear-switch',
            accessible_name: 'Ear Detection', y_align: Clutter.ActorAlign.CENTER});
        earChoices.layout_manager.homogeneous = true;
        this._earItems = EARS.map(([name], index) => {
            const item = new St.Button({style_class: 'airpods-ear-option',
                label: ['I', 'II', 'Off'][index], can_focus: true, reactive: true, track_hover: true,
                accessible_name: `Ear Detection, ${name}`, accessible_role: Atk.Role.RADIO_BUTTON});
            item.connect('clicked', () => this._selectEar(index));
            item.connect('key-press-event', (_actor, event) => {
                const key = event.get_key_symbol();
                if (key !== Clutter.KEY_Left && key !== Clutter.KEY_Right) return Clutter.EVENT_PROPAGATE;
                const direction = key === Clutter.KEY_Right ? 1 : -1;
                const next = EAR_ORDER[(EAR_ORDER.indexOf(index) + direction + EAR_ORDER.length) % EAR_ORDER.length];
                this._earItems[next].grab_key_focus();
                this._selectEar(next);
                return Clutter.EVENT_STOP;
            });
            return item;
        });
        for (const index of EAR_ORDER) earChoices.add_child(this._earItems[index]);
        this._ears.add_child(earChoices);
        this._button.menu.connect('open-state-changed', (_menu, open) => {
            if (open) this._backend?.refresh();
        });
        Main.panel.addToStatusArea(this.uuid, this._button);
        this._render(null);
    }

    _row(styleClass) {
        const item = new PopupMenu.PopupBaseMenuItem({reactive: true, activate: false, hover: false, can_focus: false,
            style_class: styleClass});
        this._button.menu.addMenuItem(item);
        return item;
    }

    _separator() {
        const item = new PopupMenu.PopupSeparatorMenuItem();
        this._button.menu.addMenuItem(item);
        return item;
    }

    _setChoice(button, selected) {
        if (selected) {
            button.add_style_pseudo_class('checked');
            button.add_accessible_state(Atk.StateType.CHECKED);
        } else {
            button.remove_style_pseudo_class('checked');
            button.remove_accessible_state(Atk.StateType.CHECKED);
        }
    }

    _batteryRing(name) {
        const row = new St.BoxLayout({orientation: Clutter.Orientation.VERTICAL,
            x_expand: true, style_class: 'airpods-battery-cell'});
        const circle = new St.Widget({layout_manager: new Clutter.BinLayout(),
            style_class: 'airpods-battery-circle', x_align: Clutter.ActorAlign.CENTER});
        const meter = new St.DrawingArea({style_class: 'airpods-ring', x_expand: true, y_expand: true,
            accessible_role: Atk.Role.LEVEL_BAR, accessible_name: `${name} battery`});
        const icon = new St.Icon({icon_size: 28, x_align: Clutter.ActorAlign.CENTER, y_align: Clutter.ActorAlign.CENTER});
        const charging = new St.Icon({gicon: this._batteryIcons.charging, icon_size: 12,
            x_expand: true, y_expand: true, x_align: Clutter.ActorAlign.END,
            y_align: Clutter.ActorAlign.END, style_class: 'airpods-charging'});
        circle.add_child(meter);
        circle.add_child(icon);
        circle.add_child(charging);
        row.add_child(circle);
        const percent = label('—', 'airpods-battery-percent');
        percent.x_align = Clutter.ActorAlign.CENTER;
        row.add_child(percent);
        const view = {row, percent, meter, icon, charging, level: null};
        meter.connect('repaint', () => this._paintBatteryRing(view));
        return view;
    }

    _paintBatteryRing(view) {
        const {meter, level} = view;
        const cr = meter.get_context();
        const [width, height] = meter.get_surface_size();
        const lineWidth = Math.min(width, height) * 0.065;
        const radius = (Math.min(width, height) - lineWidth) / 2;
        try {
            cr.setLineWidth(lineWidth);
            cr.setLineCap(Cairo.LineCap.ROUND);
            cr.setSourceRGBA(0.5, 0.5, 0.5, 0.22);
            cr.arc(width / 2, height / 2, radius, 0, Math.PI * 2);
            cr.stroke();
            if (level > 0) {
                cr.setSourceColor(meter.get_theme_node().get_color('-ring-color'));
                cr.arc(width / 2, height / 2, radius, -Math.PI / 2, -Math.PI / 2 + Math.PI * 2 * level / 100);
                cr.stroke();
            }
        } finally {
            cr.$dispose();
        }
    }

    _request(command, selection) {
        if (!this._enabled || !this._lastStatus?.connected) return false;
        if (this._backend?.command(command, selection)) return true;
        this._handleError('AirPods controls are unavailable', 'command', command, selection);
        return false;
    }

    _selectMode(mode) {
        if (!this._lastStatus?.connected || !this._lastStatus.modes.includes(mode)) return false;
        return this._selectControl('mode', mode, MODES[mode][1]);
    }

    _holdSelection(key, value, command, selection = null) {
        this._clearSelection(key);
        const pending = selection ?? {value, command, timeout: 0};
        // Keep older snapshots from undoing the user's latest input while it settles.
        pending.timeout = GLib.timeout_add(GLib.PRIORITY_DEFAULT, SELECTION_SETTLE_MS, () => {
            if (key === 'adaptive' && this._sliderDragging) return GLib.SOURCE_CONTINUE;
            pending.timeout = 0;
            this._pendingSelections.delete(key);
            this._render(this._lastStatus);
            return GLib.SOURCE_REMOVE;
        });
        this._pendingSelections.set(key, pending);
        return pending;
    }

    _selectControl(key, value, command) {
        const pending = this._holdSelection(key, value, command);
        this._render(this._lastStatus);
        return this._request(command, pending);
    }

    _clearSelection(key) {
        const pending = this._pendingSelections.get(key);
        if (pending?.timeout) GLib.Source.remove(pending.timeout);
        this._pendingSelections.delete(key);
    }

    _cancelAdaptive() {
        this._clearSelection('adaptive');
        if (this._sliderTimeout) GLib.Source.remove(this._sliderTimeout);
        this._sliderTimeout = 0;
    }

    _clearPendingControls() {
        for (const key of this._pendingSelections.keys()) this._clearSelection(key);
        this._cancelAdaptive();
        this._sliderDragging = false;
        this._scrollAmount = 0;
        this._scrollAt = 0;
    }

    _displayedValue(key) {
        return this._pendingSelections.get(key)?.value ?? this._lastStatus?.[key];
    }

    _selectConversation(enabled) {
        if (typeof enabled !== 'boolean' || !this._lastStatus?.connected || !this._lastStatus.conversationSupported) return false;
        return this._selectControl('conversation', enabled, `ca:${enabled ? 'on' : 'off'}`);
    }

    _selectEar(behavior) {
        if (!this._lastStatus?.connected || !Number.isInteger(behavior) || !EARS[behavior]) return false;
        return this._selectControl('ear', behavior, EARS[behavior][1]);
    }

    _cycleMode(direction) {
        const status = this._lastStatus;
        if (!status?.connected || !status.modes.length) return false;
        return this._selectMode(cycleMode(status.modes, this._displayedValue('mode'), direction));
    }

    _onPanelScroll(event) {
        if (!this._lastStatus?.connected || !this._lastStatus.modes.length)
            return Clutter.EVENT_PROPAGATE;
        if (event.get_flags() & Clutter.EventFlags.FLAG_POINTER_EMULATED)
            return Clutter.EVENT_PROPAGATE;
        const direction = event.get_scroll_direction();
        if (direction === Clutter.ScrollDirection.UP || direction === Clutter.ScrollDirection.DOWN) {
            this._cycleMode(direction === Clutter.ScrollDirection.DOWN ? 1 : -1);
            return Clutter.EVENT_STOP;
        }
        if (direction !== Clutter.ScrollDirection.SMOOTH) return Clutter.EVENT_PROPAGATE;
        const [, dy] = event.get_scroll_delta();
        const now = GLib.get_monotonic_time();
        if (now - this._scrollAt > 250000) this._scrollAmount = 0;
        this._scrollAt = now;
        this._scrollAmount += dy;
        if (Math.abs(this._scrollAmount) >= 1) {
            this._cycleMode(Math.sign(this._scrollAmount));
            this._scrollAmount = 0;
        }
        return Clutter.EVENT_STOP;
    }

    _handleError(message, kind, command, selection) {
        if (!this._enabled) return;
        if (kind === 'command') {
            console.warn(`AirPods command failed: ${message}`);
            for (const [key, pending] of this._pendingSelections) {
                // Repeating a choice creates a new submission even when the verb
                // matches an earlier command that is still running.
                if (selection ? selection === pending : !command || command === pending.command) {
                    if (key === 'adaptive') this._cancelAdaptive();
                    else this._clearSelection(key);
                }
            }
            this._render(this._lastStatus);
        } else {
            this._render(null);
        }
    }

    _render(status) {
        if (!this._enabled) return;
        this._lastStatus = status;
        this._syncing = true;
        try {
            if (!status?.connected) {
                this._clearPendingControls();
                this._button.menu.close();
            }
            this._button.visible = !!status?.connected;
            const icon = this._icons[status?.iconVariant ?? 'buds'];
            this._panelIcon.gicon = icon;
            this._heroIcon.gicon = icon;
            this._label.text = panelText(status);
            this._title.text = status?.model || status?.name || 'AirPods';
            this._button.accessible_name = `${this._title.text}, ${this._label.text}`;
            this._batteryGroup.visible = !!status;
            this._batteries.forEach((view, index) => {
                const entry = status?.batteries[index];
                view.row.visible = !!entry;
                if (!entry) return;
                const [name, battery] = entry;
                view.icon.gicon = name === 'Headphones' ? this._icons.max : name === 'Case' ? this._batteryIcons.case
                    : this._batteryIcons[`${status.iconVariant === 'pro' ? 'pro' : 'buds'}-${name.toLowerCase()}`];
                view.percent.text = battery.level === null ? '—' : `${battery.level}%`;
                const levelChanged = view.level !== battery.level;
                view.level = battery.level;
                view.charging.visible = battery.charging;
                view.meter.accessible_name = `${name} battery, ${battery.level === null ? 'unknown' : `${battery.level} percent`}, ${batteryDetail(name, battery)}`;
                if (battery.level !== null && battery.level <= 20 && !battery.charging)
                    view.meter.add_style_class_name('airpods-battery-low');
                else view.meter.remove_style_class_name('airpods-battery-low');
                if (levelChanged) view.meter.queue_repaint();
            });
            const hasModes = !!status?.connected && status.modes.length > 0;
            this._listeningSeparator.visible = hasModes;
            this._modeButtonsRow.visible = hasModes;
            // Let longer labels use more space when all four modes are available.
            this._modes[0].get_parent().layout_manager.homogeneous = status?.modes.length !== 4;
            if (!hasModes || !status.modes.includes(this._displayedValue('mode')))
                this._clearSelection('mode');
            const displayedMode = this._displayedValue('mode');
            this._modes.forEach((item, index) => {
                item.visible = hasModes && status.modes.includes(index);
                this._setChoice(item, displayedMode === index);
            });
            this._adaptive.visible = !!status?.connected && status.modes.includes(3) && displayedMode === 3;
            if (!this._adaptive.visible) {
                this._cancelAdaptive();
                this._sliderDragging = false;
            }
            if (!this._pendingSelections.has('adaptive') && !this._sliderDragging)
                this._slider.value = (status?.adaptive ?? 50) / 100;
            this._adaptiveValue.text = `${Math.round(this._slider.value * 100)}%`;
            this._optionsSeparator.visible = !!status?.connected;
            this._conversation.visible = !!status?.connected && status.conversationSupported;
            if (!this._conversation.visible) this._clearSelection('conversation');
            const conversationEnabled = this._displayedValue('conversation') === true;
            if (this._conversationSwitch.state !== conversationEnabled)
                this._conversationSwitch.state = conversationEnabled;
            this._setChoice(this._conversationToggle, conversationEnabled);
            this._ears.visible = !!status?.connected;
            const ear = this._displayedValue('ear');
            this._earItems.forEach((item, index) => this._setChoice(item, ear === index));
        } finally {
            this._syncing = false;
        }
    }

    disable() {
        this._enabled = false;
        this._clearPendingControls();
        this._backend?.destroy();
        this._backend = null;
        this._button?.destroy();
        this._button = null;
        for (const key of ['_label', '_title', '_batteries', '_modes', '_earItems',
            '_adaptive', '_adaptiveValue', '_slider', '_conversation', '_ears',
            '_conversationToggle', '_conversationSwitch',
            '_batteryGroup',
            '_listeningSeparator', '_optionsSeparator', '_heroIcon', '_lastStatus',
            '_modeButtonsRow', '_panelIcon', '_icons', '_batteryIcons'])
            this[key] = null;
    }
}
