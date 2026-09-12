import test from 'node:test';
import assert from 'node:assert/strict';
import {parseStatus, panelText, batteryDetail, validCommand, cycleMode} from '../model.js';

const parse = values => parseStatus(JSON.stringify({schema_version: 1, ...values}));

test('rejects malformed and incompatible status instead of displaying stale values', () => {
    for (const text of ['', '{', 'null', '[]', '{}', 'true', '1', '"AirPods"',
        '{"schema_version":"1"}', '{"schema_version":2}'])
        assert.throws(() => parseStatus(text));
});
test('unavailable batteries clear stale charging, ear and level data', () => {
    const status = parse({left: {available: false, level: 80, charging: true, in_ear: true}});
    assert.deepEqual(status.batteries[0][1], {level: null, charging: false, inEar: false});
    assert.equal(panelText(status), 'AirPods');
});
test('panel uses lower available earbud level, excluding the case', () => {
    const status = parse({left: {available: true, level: 73}, right: {available: true, level: 41}, case: {available: true, level: 3}});
    assert.equal(panelText(status), '41%');
});
test('headsets show one battery without phantom earbuds or case', () => {
    const status = parse({is_headset: true, headset: {available: true, level: 92}});
    assert.equal(status.batteries.length, 1);
    assert.equal(status.batteries[0][0], 'Headphones');
    assert.equal(panelText(status), '92%');
});

test('icon variant follows the device family, with headset taking priority', () => {
    assert.equal(parse({}).iconVariant, 'buds');
    assert.equal(parse({is_pro_series: true}).iconVariant, 'pro');
    assert.equal(parse({is_headset: true, is_pro_series: true}).iconVariant, 'max');
});
test('capabilities gate controls, including models without Off or Adaptive', () => {
    assert.deepEqual(parse({}).modes, []);
    assert.deepEqual(parse({supports_noise_control: false, supports_adaptive: true}).modes, []);
    assert.deepEqual(parse({supports_noise_control: true, supports_noise_off: false, supports_adaptive: true}).modes, [2, 3, 1]);
    assert.deepEqual(parse({supports_noise_control: true, supports_noise_off: true}).modes, [0, 2, 1]);
});
test('invalid levels are unknown, and BLE battery can display while disconnected', () => {
    for (const level of [-1, 101, 50.5, '50', null, undefined, false, {}])
        assert.equal(parse({left: {available: true, level}}).batteries[0][1].level, null);
    assert.equal(panelText(parse({connected: false, left: {available: true, level: 0}})), '0%');
});
test('only known control verbs and bounded adaptive values are accepted', () => {
    for (const verb of ['noise:off', 'noise:anc', 'noise:transparency', 'noise:adaptive',
        'ca:on', 'ca:off', 'ear:one', 'ear:both', 'ear:off'])
        assert.equal(validCommand(verb), true);
    for (let level = 0; level <= 100; level++)
        assert.equal(validCommand(`adaptive:${level}`), true);
    for (const verb of ['reboot', 'adaptive:101', 'adaptive:-1', 'noise:anc;whoami', 'ca:yes',
        'adaptive:', 'adaptive:01', 'adaptive:+1', 'adaptive:-0', 'adaptive:1.0',
        'adaptive:1e1', 'adaptive:0x10', 'adaptive:NaN', 'adaptive:Infinity',
        'adaptive: 1', ' adaptive:1', 'adaptive:1 ', 'adaptive:1\n', 'adaptive:1\r\n',
        'ca:on\n', 'ca:off\u2028', 'noise:anc\n', 'ear:off\0', 'anc_one:on'])
        assert.equal(validCommand(verb), false, JSON.stringify(verb));
});

test('command validation rejects non-strings without coercing them', () => {
    for (const value of [undefined, null, true, 42, [], ['ca:on'], new String('ca:on'),
        {toString() { throw new Error('Command coercion must not run'); }}])
        assert.equal(validCommand(value), false);
});

test('left, right and case remain independent, including a missing case reading', () => {
    const status = parse({left: {available: true, level: 76}, right: {available: true, level: 41},
        case: {available: false, level: 0}});
    assert.deepEqual(status.batteries.map(([, battery]) => battery.level), [76, 41, null]);
    assert.equal(batteryDetail('Case', status.batteries[2][1]), 'Waiting for case data');
    const emptyCase = parse({case: {available: true, level: 0}}).batteries[2][1];
    assert.equal(emptyCase.level, 0);
    assert.equal(batteryDetail('Case', emptyCase), '');
});

test('battery availability and flags require boolean values', () => {
    for (const available of [undefined, null, false, 1, 'true']) {
        const result = parse({left: {available, level: 42, charging: true, in_ear: true}});
        assert.deepEqual(result.batteries[0][1], {level: null, charging: false, inEar: false});
    }
    const result = parse({left: {available: true, level: 100, charging: 'true', in_ear: 1}});
    assert.deepEqual(result.batteries[0][1], {level: 100, charging: false, inEar: false});
});

test('accessible battery descriptions preserve charging and in-ear states', () => {
    assert.equal(batteryDetail('Left', {level: 50, charging: true, inEar: true}), 'Charging');
    assert.equal(batteryDetail('Right', {level: 50, charging: false, inEar: true}), 'In ear');
    assert.equal(batteryDetail('Left', {level: null, charging: false, inEar: false}), 'Unavailable');
});

test('adaptive levels are bounded and unknown readings use the neutral default', () => {
    for (const [level, expected] of [[-1, 0], [0, 0], [37, 37], [100, 100], [101, 100]])
        assert.equal(parse({adaptive_noise_level: level}).adaptive, expected);
    for (const level of [undefined, null, '37', 37.5, true, {}])
        assert.equal(parse({adaptive_noise_level: level}).adaptive, 50);
});

test('mode cycling wraps in each direction and skips unsupported modes', () => {
    assert.equal(cycleMode([2, 3, 1], 1, 1), 2);
    assert.equal(cycleMode([2, 3, 1], 2, -1), 1);
    assert.equal(cycleMode([0, 2, 1], 2, 1), 1);
    assert.equal(cycleMode([2, 3, 1], -1, 1), 2);
    assert.equal(cycleMode([2, 3, 1], -1, -1), 1);
    assert.equal(cycleMode([], 1, 1), null);
    assert.equal(cycleMode([1], 1, 0), null);
});
