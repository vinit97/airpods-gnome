// Run inside gnome-shell-test-tool, with a separate session bus and test state directory.
import Gio from 'gi://Gio';
import GLib from 'gi://GLib';
import Shell from 'gi://Shell';
import St from 'gi://St';
import Clutter from 'gi://Clutter';
import * as Main from 'resource:///org/gnome/shell/ui/main.js';
import * as Scripting from 'resource:///org/gnome/shell/ui/scripting.js';

function assert(condition, message) { if (!condition) throw new Error(message); }

export async function run() {
    await Scripting.sleep(1000);
    const extension = Main.extensionManager.lookup(GLib.getenv('AIRPODS_EXTENSION_UUID'))?.stateObj;
    assert(extension?._button, 'Extension failed to load');
    extension._backend.destroy();
    const commands = [];
    const selections = [];
    const recordCommand = (command, selection) => {
        commands.push(command);
        selections.push(selection);
        return true;
    };
    extension._backend.command = recordCommand;
    const {parseStatus} = await import(`file://${extension.path}/model.js`);
    const status = parseStatus(JSON.stringify({schema_version: 1, connected: true,
        model_name: 'AirPods Pro 3', device_name: 'AirPods', is_pro_series: true, supports_noise_control: true,
        supports_noise_off: false, supports_adaptive: true, noise_mode: 1,
        supports_conversational_awareness: true, conversational_awareness: true,
        ear_detection_behavior: 0, adaptive_noise_level: 50,
        left: {available: true, level: 76, in_ear: true},
        right: {available: true, level: 41, in_ear: true},
        case: {available: false, level: 0}}));
    extension._render({...status, conversation: false});
    extension._render(status);
    assert(extension._heroIcon.gicon.equal(extension._icons.pro), 'AirPods Pro artwork was not selected');
    assert(extension._panelIcon.gicon.equal(extension._icons.pro), 'Top-bar artwork differs from the menu');
    assert(commands.length === 0, 'Rendering status sent Bluetooth commands');
    assert(extension._batteries[0].percent.text === '76%', 'Left battery was mapped incorrectly');
    assert(extension._batteries[1].percent.text === '41%', 'Right battery was mapped incorrectly');
    assert(extension._batteries[2].row.visible, 'Case row disappeared');
    assert(extension._batteries[2].percent.text === '—', 'Unavailable case must show an unknown value');
    const batteryOrder = extension._batteries[0].row.get_parent().get_children();
    assert(batteryOrder[0] === extension._batteries[0].row && batteryOrder[1] === extension._batteries[2].row
        && batteryOrder[2] === extension._batteries[1].row, 'Battery rings must be ordered left, case, right');
    assert(extension._batteries[0].icon.gicon.equal(extension._batteryIcons['pro-left'])
        && extension._batteries[1].icon.gicon.equal(extension._batteryIcons['pro-right'])
        && extension._batteries[2].icon.gicon.equal(extension._batteryIcons.case), 'Battery artwork was mapped incorrectly');
    assert(extension._batteries[2].level === null, 'Unknown case ring pretended to have a percentage');
    extension._handleError('Test command failed', 'command');
    assert(extension._batteries[0].percent.text === '76%' && extension._batteries[0].row.visible,
        'Command failure erased valid battery readings');
    const assertNoErrorUi = () => {
        assert(!extension._errorRow && !extension._errorLabel && !extension._commandError,
            'An error message was added to the menu');
        assert(!extension._button.menu._getMenuItems().some(item =>
            item.has_style_class_name('airpods-message-row')), 'An error row was added to the menu');
    };
    assertNoErrorUi();
    extension._backend.command = () => false;
    assert(!extension._selectMode(2), 'Rejected dispatch was reported as accepted');
    assert(!extension._pendingSelections.has('mode') && extension._modes[1].has_style_pseudo_class('checked'),
        'Rejected dispatch left a false mode highlight');
    assert(extension._batteries[0].percent.text === '76%',
        'Rejected dispatch did not preserve battery data');
    assertNoErrorUi();
    extension._backend.command = recordCommand;
    extension._render(status);
    Main.overview.hide();
    extension._button.menu.open();
    await Scripting.sleep(300);
    const screenshotDir = GLib.getenv('AIRPODS_SCREENSHOT_DIR');
    {
        const previewStatus = {...status, batteries: status.batteries.map(([name, battery]) =>
            [name, name === 'Case' ? {...battery, level: 68} : battery])};
        extension._render(previewStatus);
        assert(extension._batteries[2].percent.text === '68%', 'Reported case battery was not displayed');
        await Scripting.sleep(100);
        const takeScreenshot = async name => {
            if (!screenshotDir) return;
            const actor = extension._button.menu.actor;
            const [x, y] = actor.get_transformed_position();
            const [width, height] = actor.get_transformed_size();
            const stream = Gio.File.new_for_path(`${screenshotDir}/${name}.png`).replace(null, false, Gio.FileCreateFlags.NONE, null);
            try {
                await new Shell.Screenshot().screenshot_area(Math.floor(x), Math.floor(y),
                    Math.ceil(width), Math.ceil(height), stream);
            } finally {
                stream.close(null);
            }
        };
        await takeScreenshot('menu');
        extension._render({...previewStatus, ear: 1});
        await Scripting.sleep(100);
        await takeScreenshot('menu-ear-both');
        extension._render(previewStatus);
        const context = St.ThemeContext.get_for_stage(global.stage);
        const darkTheme = context.get_theme();
        const lightTheme = new St.Theme({default_stylesheet:
            Gio.File.new_for_uri('resource:///org/gnome/shell/theme/gnome-shell-light.css')});
        for (const sheet of darkTheme.get_custom_stylesheets()) lightTheme.load_stylesheet(sheet);
        context.set_theme(lightTheme);
        await Scripting.sleep(200);
        await takeScreenshot('menu-light');
        context.set_theme(darkTheme);
        extension._render({...previewStatus, modes: [0, 2, 3, 1]});
        await Scripting.sleep(100);
        for (const button of extension._modes) {
            const caption = button.get_child().get_last_child();
            assert(!caption.clutter_text.get_layout().is_ellipsized(), 'Four-mode layout truncated a label');
        }
        await takeScreenshot('menu-four-modes');
        extension._render({...status, mode: 3, batteries: status.batteries.map(([name, battery]) =>
            [name, name === 'Case' ? {...battery, level: 68, charging: true} : battery])});
        await Scripting.sleep(100);
        await takeScreenshot('menu-adaptive');
        const leftMeter = extension._batteries[0].meter;
        let repaints = 0;
        const repaintSignal = leftMeter.connect('repaint', () => { repaints++; });
        const lowBatteryStatus = {...status, batteries: status.batteries.map(([name, battery]) =>
            [name, name === 'Left' ? {...battery, level: 10, charging: false} : battery])};
        extension._render(lowBatteryStatus);
        await Scripting.sleep(100);
        const lowColor = leftMeter.get_theme_node().get_color('-ring-color').to_string();
        const beforeCharging = repaints;
        extension._render({...lowBatteryStatus, batteries: lowBatteryStatus.batteries.map(([name, battery]) =>
            [name, name === 'Left' ? {...battery, charging: true} : battery])});
        await Scripting.sleep(100);
        assert(repaints > beforeCharging && leftMeter.get_theme_node().get_color('-ring-color').to_string() !== lowColor,
            'A charging change at the same percentage did not repaint the low battery ring');
        leftMeter.disconnect(repaintSignal);
        extension._render(status);
        await Scripting.sleep(100);
    }
    const [, compactHeight] = extension._button.menu.actor.get_transformed_size();
    assert(compactHeight < 390, `Menu is no longer compact: ${compactHeight}px`);
    assert(extension._modes[0].visible === false, 'Unsupported Off mode is shown');
    assert(extension._modes[1].has_style_pseudo_class('checked'), 'Current listening mode is not highlighted');
    assert(extension._conversationSwitch.state, 'Conversation Awareness switch is not on');
    assert(extension._earItems[0].has_style_pseudo_class('checked'), 'Current ear-detection option is not selected');
    const earOrder = extension._earItems[0].get_parent().get_children();
    assert(earOrder.map(item => item.label).join(',') === 'Off,I,II',
        'Ear switch positions are not labeled and ordered Off, I, II');
    const [, earHeight] = extension._ears.get_transformed_size();
    assert(earHeight < 48, `Ear switch no longer fits in a compact row: ${earHeight}px`);

    extension._button.menu.close();
    const pointer = Clutter.get_default_backend().get_default_seat().create_virtual_device(Clutter.InputDeviceType.POINTER_DEVICE);
    // Let the new pointer enter the stage before clicking any Shell controls.
    pointer.notify_absolute_motion(GLib.get_monotonic_time(), 300, 300);
    await Scripting.sleep(300);
    const click = async (actor, button) => {
        await Scripting.sleep(100);
        const [x, y] = actor.get_transformed_position();
        const [width, height] = actor.get_transformed_size();
        pointer.notify_absolute_motion(GLib.get_monotonic_time(), x + width / 2, y + height / 2);
        await Scripting.sleep(30);
        pointer.notify_button(GLib.get_monotonic_time(), button, Clutter.ButtonState.PRESSED);
        await Scripting.sleep(30);
        pointer.notify_button(GLib.get_monotonic_time(), button, Clutter.ButtonState.RELEASED);
        await Scripting.sleep(80);
    };
    await click(extension._button, Clutter.BUTTON_PRIMARY);
    assert(extension._button.menu.isOpen, 'Pointer could not open the menu for feature choices');
    await click(extension._conversationToggle, Clutter.BUTTON_PRIMARY);
    assert(commands.length === 1 && commands[0] === 'ca:off', `Conversation Off did not send exactly one command: ${JSON.stringify(commands)}`);
    assert(extension._button.menu.isOpen, 'Conversation choice closed the menu');
    assert(!extension._conversationSwitch.state, 'Conversation switch did not respond immediately');
    extension._render(status);
    assert(!extension._conversationSwitch.state, 'Stale status undid the Conversation selection');
    extension._render({...status, conversation: false});
    assert(!extension._conversationSwitch.state, 'Confirmed Off state was not displayed');
    assert(!extension._conversationToggle.has_style_pseudo_class('checked'), 'Conversation button and switch disagree');
    await click(extension._conversationToggle, Clutter.BUTTON_PRIMARY);
    assert(commands.at(-1) === 'ca:on', 'Conversation On sent the wrong command');
    extension._handleError('Earlier Conversation command failed', 'command', 'ca:off');
    assert(extension._conversationSwitch.state, 'Earlier failure undid the newest Conversation choice');
    extension._render(status);
    extension._selectConversation(false);
    extension._handleError('Conversation command failed', 'command', 'ca:off');
    assert(extension._conversationSwitch.state, 'Conversation failure did not restore service state');
    for (const [index, command] of [[2, 'ear:off'], [0, 'ear:one'], [1, 'ear:both']]) {
        const before = commands.length;
        await click(extension._earItems[index], Clutter.BUTTON_PRIMARY);
        assert(commands.length === before + 1 && commands.at(-1) === command,
            `Ear-detection position ${index} did not send exactly one correct command`);
        assert(extension._earItems[index].has_style_pseudo_class('checked'), 'Ear selection did not highlight immediately');
        extension._render({...status, ear: (index + 1) % 3});
        assert(extension._earItems[index].has_style_pseudo_class('checked'), 'Stale status undid the ear-detection selection');
        extension._render({...status, ear: index});
        assert(extension._earItems[index].has_style_pseudo_class('checked'), 'Confirmed ear state was not displayed');
        assert(extension._earItems.filter(item => item.has_style_pseudo_class('checked')).length === 1,
            'Multiple ear-detection choices are selected');
        assert(extension._button.menu.isOpen, 'Ear-detection choice closed the menu');
    }
    extension._render(status);
    extension._selectEar(1);
    extension._handleError('Ear command failed', 'command', 'ear:both');
    assert(!extension._pendingSelections.has('ear') && extension._earItems[0].has_style_pseudo_class('checked'),
        'Ear failure did not restore the reported position');
    const beforePassiveUpdate = commands.length;
    extension._render(status);
    assert(commands.length === beforePassiveUpdate, 'Updating choice state sent an extra command');
    extension._render({...status, conversationSupported: false});
    assert(!extension._conversation.visible, 'Unsupported Conversation Awareness is visible');
    assert(!extension._selectConversation(true), 'Unsupported Conversation Awareness sent a command');
    assert(!extension._selectConversation('off'), 'Invalid Conversation Awareness value was accepted');
    assert(!extension._selectEar(5), 'Invalid ear-detection choice was accepted');
    extension._render(status);

    extension._button.menu.close();
    await click(extension._button, Clutter.BUTTON_SECONDARY);
    assert(commands.at(-1) === 'noise:transparency', 'Right-click did not cycle to the next supported mode');
    assert(!extension._button.menu.isOpen, 'Right-click opened the menu');
    pointer.notify_discrete_scroll(GLib.get_monotonic_time(), Clutter.ScrollDirection.DOWN, Clutter.ScrollSource.WHEEL);
    await Scripting.sleep(80);
    assert(commands.at(-1) === 'noise:adaptive', 'Scroll did not advance from the pending mode');
    pointer.notify_discrete_scroll(GLib.get_monotonic_time(), Clutter.ScrollDirection.UP, Clutter.ScrollSource.WHEEL);
    await Scripting.sleep(80);
    assert(commands.at(-1) === 'noise:transparency', 'Scroll up did not select the previous mode');
    await click(extension._button, Clutter.BUTTON_PRIMARY);
    assert(extension._button.menu.isOpen, 'Normal left click stopped opening the menu');
    await click(extension._modes[1], Clutter.BUTTON_PRIMARY);
    assert(commands.at(-1) === 'noise:anc', 'Mode button did not send a command');
    assert(extension._modes[1].has_style_pseudo_class('checked'), 'Mode highlight did not update immediately');
    assert(extension._button.menu.isOpen, 'Mode button unexpectedly closed the menu');
    assert(!extension._selectMode(0), 'Unsupported mode was accepted');

    extension._modes[3].grab_key_focus();
    const keyboard = Clutter.get_default_backend().get_default_seat().create_virtual_device(Clutter.InputDeviceType.KEYBOARD_DEVICE);
    keyboard.notify_keyval(GLib.get_monotonic_time(), Clutter.KEY_space, Clutter.KeyState.PRESSED);
    await Scripting.sleep(30);
    keyboard.notify_keyval(GLib.get_monotonic_time(), Clutter.KEY_space, Clutter.KeyState.RELEASED);
    await Scripting.sleep(80);
    assert(commands.at(-1) === 'noise:adaptive', 'Keyboard activation did not select the focused mode');
    assert(extension._modes[3].has_style_pseudo_class('checked') && extension._adaptive.visible,
        'Keyboard selection did not update the mode and adaptive slider immediately');

    extension._conversationToggle.grab_key_focus();
    keyboard.notify_keyval(GLib.get_monotonic_time(), Clutter.KEY_space, Clutter.KeyState.PRESSED);
    await Scripting.sleep(30);
    keyboard.notify_keyval(GLib.get_monotonic_time(), Clutter.KEY_space, Clutter.KeyState.RELEASED);
    await Scripting.sleep(80);
    assert(commands.at(-1) === 'ca:off', 'Keyboard could not turn Conversation Awareness off');
    extension._earItems[2].grab_key_focus();
    keyboard.notify_keyval(GLib.get_monotonic_time(), Clutter.KEY_space, Clutter.KeyState.PRESSED);
    await Scripting.sleep(30);
    keyboard.notify_keyval(GLib.get_monotonic_time(), Clutter.KEY_space, Clutter.KeyState.RELEASED);
    await Scripting.sleep(80);
    assert(commands.at(-1) === 'ear:off', 'Keyboard could not turn ear detection off');
    assert(extension._button.menu.isOpen, 'Keyboard selection closed the menu');
    for (const [key, index, command] of [[Clutter.KEY_Right, 0, 'ear:one'],
        [Clutter.KEY_Right, 1, 'ear:both'], [Clutter.KEY_Right, 2, 'ear:off'],
        [Clutter.KEY_Left, 1, 'ear:both']]) {
        const before = commands.length;
        keyboard.notify_keyval(GLib.get_monotonic_time(), key, Clutter.KeyState.PRESSED);
        await Scripting.sleep(30);
        keyboard.notify_keyval(GLib.get_monotonic_time(), key, Clutter.KeyState.RELEASED);
        await Scripting.sleep(80);
        assert(commands.length === before + 1 && commands.at(-1) === command,
            'Arrow navigation sent the wrong ear-detection command');
        assert(global.stage.get_key_focus() === extension._earItems[index]
            && extension._earItems[index].has_style_pseudo_class('checked'),
            'Arrow navigation did not focus and select the next position');
    }

    extension._render({...status, mode: 3});
    const assertSlider = (value, message) => assert(Math.abs(extension._slider.value * 100 - value) < 0.01
        && extension._adaptiveValue.text === `${Math.round(value)}%`, message);
    extension._slider.value = 0.75;
    assertSlider(75, 'Adaptive percentage did not respond immediately');
    extension._render({...status, mode: 3});
    assertSlider(75, 'Status update moved the slider during debounce');
    await Scripting.sleep(250);
    assert(commands.includes('adaptive:75'), 'Slider did not send the final level');
    extension._render({...status, mode: 3});
    assertSlider(75, 'Older status undid the adaptive level after command dispatch');
    const beforeSubPercentMotion = commands.length;
    const sliderSelection = extension._pendingSelections.get('adaptive');
    extension._slider.value = 0.751;
    extension._slider.value = 0.754;
    await Scripting.sleep(250);
    assert(commands.length === beforeSubPercentMotion && extension._pendingSelections.get('adaptive') === sliderSelection,
        'Sub-percent slider motion resubmitted the same level or replaced the selection');
    extension._slider.emit('drag-begin');
    extension._slider.emit('drag-end');
    assert(extension._pendingSelections.get('adaptive') === sliderSelection,
        'Ending a slider drag lost the submitted selection identity');
    extension._handleError('Adaptive command failed after release', 'command', 'adaptive:75', sliderSelection);
    assertSlider(50, 'A submitted slider failure after release did not restore the reported level');
    const beforeRapidSlider = commands.length;
    extension._slider.value = 0.60;
    extension._slider.value = 0.83;
    assertSlider(83, 'Rapid slider input did not show the newest level');
    await Scripting.sleep(250);
    assert(commands.length === beforeRapidSlider + 1 && commands.at(-1) === 'adaptive:83', 'Slider failed to debounce to the newest level');
    extension._render({...status, mode: 3, adaptive: 60});
    extension._handleError('Older adaptive command failed', 'command', 'adaptive:60');
    extension._handleError('Unrelated command failed', 'command', 'noise:anc');
    assertSlider(83, 'An older or unrelated failure undid the adaptive choice');
    extension._handleError('Adaptive command failed', 'command', 'adaptive:83');
    assertSlider(60, 'Adaptive failure did not restore the reported level');

    // Exercise an actual pointer drag, including a status update while holding the thumb.
    extension._render({...status, mode: 3});
    await Scripting.sleep(150);
    const [sliderX, sliderY] = extension._slider.get_transformed_position();
    const [sliderWidth, sliderHeight] = extension._slider.get_transformed_size();
    pointer.notify_absolute_motion(GLib.get_monotonic_time(), sliderX + sliderWidth / 2, sliderY + sliderHeight / 2);
    pointer.notify_button(GLib.get_monotonic_time(), Clutter.BUTTON_PRIMARY, Clutter.ButtonState.PRESSED);
    await Scripting.sleep(50);
    pointer.notify_absolute_motion(GLib.get_monotonic_time(), sliderX + sliderWidth * 0.8, sliderY + sliderHeight / 2);
    await Scripting.sleep(100);
    assert(extension._sliderDragging, 'Pointer did not begin an adaptive slider drag');
    const draggedLevel = extension._slider.value * 100;
    assert(draggedLevel > 75, 'Pointer drag did not move the slider');
    extension._render({...status, mode: 3, adaptive: 25});
    assertSlider(draggedLevel, 'Service update moved the slider away from the pointer');
    await Scripting.sleep(4100);
    assertSlider(draggedLevel, 'Settling timer moved the thumb during a held drag');
    pointer.notify_button(GLib.get_monotonic_time(), Clutter.BUTTON_PRIMARY, Clutter.ButtonState.RELEASED);
    await Scripting.sleep(50);
    assert(!extension._sliderDragging, 'Slider drag did not end');
    // Feature choices use the same settling timeout as the slider.
    extension._selectConversation(false);
    extension._selectEar(2);
    extension._render({...status, mode: 3, adaptive: 25});
    await Scripting.sleep(4100);
    assertSlider(25, 'Unconfirmed adaptive level did not return to service state');
    assert(extension._conversationSwitch.state
        && extension._earItems[0].has_style_pseudo_class('checked'), 'Feature selections did not settle to reported state');
    extension._slider.value = 0.92;
    await Scripting.sleep(250);
    extension._render({...status, mode: 3, adaptive: 92});
    await Scripting.sleep(3900);
    assertSlider(92, 'Confirmed adaptive level was lost after settling');
    extension._slider.emit('drag-begin');
    extension._slider.value = 0.65;
    await Scripting.sleep(250);
    extension._handleError('Adaptive drag command failed', 'command', 'adaptive:65');
    assertSlider(65, 'A command error moved the thumb during the drag');
    extension._slider.emit('drag-end');
    assertSlider(92, 'A failed slider drag did not restore the reported level on release');
    extension._slider.value = 0.30;
    const beforeModeHidesSlider = commands.length;
    extension._selectMode(2);
    await Scripting.sleep(250);
    assert(!extension._pendingSelections.has('adaptive') && !extension._sliderTimeout,
        'Hiding adaptive mode left pending slider work');
    assert(!commands.slice(beforeModeHidesSlider).includes('adaptive:30'), 'Hidden slider dispatched a delayed command');

    extension._selectMode(2);
    assert(extension._modes[2].has_style_pseudo_class('checked'), 'Selection waited for the service');
    extension._render({...status, mode: 1});
    assert(extension._modes[2].has_style_pseudo_class('checked'), 'An older status snapshot undid the click');
    extension._selectMode(3);
    extension._render({...status, mode: 2});
    assert(extension._modes[3].has_style_pseudo_class('checked'), 'An intermediate reply replaced the newest selection');
    extension._handleError('Earlier mode failed', 'command', 'noise:transparency');
    assert(extension._modes[3].has_style_pseudo_class('checked'), 'An earlier command failure undid the newest selection');
    extension._handleError('Unrelated control failed', 'command', 'ca:on');
    assert(extension._modes[3].has_style_pseudo_class('checked'), 'An unrelated failure undid the mode selection');
    extension._handleError('Mode failed', 'command', 'noise:adaptive');
    assert(extension._modes[2].has_style_pseudo_class('checked') && !extension._pendingSelections.has('mode'),
        'Failed selection did not restore the reported mode');
    extension._render({...status, mode: 1});
    extension._selectMode(2);
    const earlierSelection = selections.at(-1);
    extension._selectMode(3);
    extension._selectMode(2);
    const latestSelection = selections.at(-1);
    extension._handleError('Earlier identical mode failed', 'command', 'noise:transparency', earlierSelection);
    assert(extension._modes[2].has_style_pseudo_class('checked') && extension._pendingSelections.has('mode'),
        'An earlier identical command failure undid the newest selection');
    extension._handleError('Latest mode failed', 'command', 'noise:transparency', latestSelection);
    assert(extension._modes[1].has_style_pseudo_class('checked') && !extension._pendingSelections.has('mode'),
        'The newest repeated command failure did not restore the reported mode');
    extension._render({...status, mode: 2});
    extension._selectMode(1);
    await Scripting.sleep(4100);
    assert(extension._modes[2].has_style_pseudo_class('checked') && !extension._pendingSelections.has('mode'),
        'Unconfirmed selection remained highlighted after timeout');
    extension._selectMode(3);
    extension._render({...status, mode: 3});
    await Scripting.sleep(4100);
    assert(extension._modes[3].has_style_pseudo_class('checked'), 'Confirmed selection was lost after timeout');
    extension._selectConversation(false);
    extension._selectEar(2);
    extension._slider.value = 0.35;
    extension._handleError('Prior command failed', 'command', 'ca:on');
    extension._button.menu.open();
    extension._render({...status, connected: false});
    assert(!extension._button.visible, 'Disconnected AirPods left an indicator in the top bar');
    assert(!extension._button.menu.isOpen, 'Disconnected AirPods left the popup open');
    assert(extension._pendingSelections.size === 0 && !extension._sliderTimeout, 'Disconnect left pending controls');
    assert(!extension._conversation.visible && !extension._ears.visible, 'Disconnected controls remained active');
    const count = commands.length;
    assert(!extension._cycleMode(1), 'Disconnected shortcut was accepted');
    assert(!extension._selectConversation(true) && !extension._selectEar(0), 'Disconnected feature choice was accepted');
    assert(commands.length === count, 'Disconnected shortcut sent a command');
    extension._render(status);
    assert(extension._button.visible, 'Reconnecting did not restore the indicator');
    assertNoErrorUi();
    for (const variant of ['buds', 'pro', 'max']) {
        extension._render({...status, iconVariant: variant});
        assert(extension._panelIcon.gicon.equal(extension._icons[variant]), `Wrong ${variant} panel icon`);
        assert(extension._heroIcon.gicon.equal(extension._icons[variant]), `Wrong ${variant} menu icon`);
    }
    extension._render({...status, iconVariant: 'max', batteries: [['Headphones', {level: 88, charging: false, inEar: false}]]});
    assert(extension._batteries[0].icon.gicon.equal(extension._icons.max)
        && extension._batteries[0].percent.text === '88%' && !extension._batteries[1].row.visible
        && !extension._batteries[2].row.visible, 'AirPods Max did not show a single headphone battery ring');
    extension._render(null);
    assert(!extension._button.visible, 'Missing service left an indicator visible');
    extension._render({...status, mode: 3});
    extension._selectMode(3);
    extension._selectConversation(false);
    extension._selectEar(2);
    extension._slider.value = 0.45;
    const beforeDisable = commands.length;
    extension.disable();
    assert(!extension._button && !extension._backend && !extension._sliderTimeout && !extension._pendingSelections.size, 'Disable did not clean up');
    await Scripting.sleep(250);
    assert(commands.length === beforeDisable, 'Disabled extension dispatched a pending slider command');
    extension._render(status);
    extension._handleError('Late callback', 'command');
    assert(!extension._button, 'A late callback recreated the disabled indicator');
    extension.disable();

    const batteryRing = extension._batteryRing;
    extension._batteryRing = () => { throw new Error('Simulated menu construction failure'); };
    let failedToEnable = false;
    try {
        extension.enable();
    } catch {
        failedToEnable = true;
    } finally {
        extension._batteryRing = batteryRing;
    }
    assert(failedToEnable && !extension._enabled && !extension._button && !extension._backend,
        'Failed enable left a partially constructed extension');
    extension.enable();
    assert(extension._button, 'Re-enable failed');
    extension.disable();
    print('PASS: Compact battery rings, immediate controls, slider drag and debounce, stale replies, failure rollback, settling timeout, shortcuts, disconnect cleanup, rejected commands, failed enable, lifecycle');
}
