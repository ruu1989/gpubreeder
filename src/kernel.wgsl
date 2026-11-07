// One generation of search. Each invocation owns one prefix of neighbours and walks every way
// of completing it. A genotype is claimed once, by the first group that can produce it.
// The CPU re-checks every claim, so this file only has to be fast and complete.

struct Params {
    pool_len: u32,
    group_size: u32,
    pooled: u32,       // 1: one workgroup shares a prefix and adds two plants
    prefix_count: u32,
    budget: u32,
    pad0: u32,
    pad1: u32,
    pad2: u32,
}

const WANT_FREE: u32 = 0u;
const CLAIMED: u32 = 2u;
const NO_CENTRE: u32 = 0xFFFFFFFFu;
const PREFIX_STRIDE: u32 = 14u;  // six packed slots, first id, four words of 16-bit ids, cost, tail range
const RECIPE_STRIDE: u32 = 6u;   // four words of 16-bit ids, centre + 1, group size

@group(0) @binding(0) var<uniform> params: Params;
// Six packed slots per plant, four bits per gene.
@group(0) @binding(1) var<storage, read> pool: array<u32>;
@group(0) @binding(2) var<storage, read> prefixes: array<u32>;
@group(0) @binding(3) var<storage, read_write> claims: array<atomic<u32>>;
@group(0) @binding(4) var<storage, read_write> recipes: array<u32>;
// Six allowed-gene masks, then one bit per genotype. Refreshed between batches, so it may be
// slightly stale, which only means a claim attempt that fails.
@group(0) @binding(5) var<storage, read> want: array<u32>;
// 65 offsets, then pool indexes. For each set of slots a centre can hold, only centres that
// differ within those slots give different results, so only those are listed.
@group(0) @binding(6) var<storage, read> centres: array<u32>;
// Crossings needed to make each plant. The pool is sorted by this.
@group(0) @binding(7) var<storage, read> costs: array<u32>;

var<private> ids: array<u32, 8>;
var<private> top: array<u32, 6>;
var<private> winners: array<u32, 6>;
var<private> choices: array<u32, 6>;
var<private> allowed: array<u32, 6>;
var<private> rules_a: array<u32, 24>;
var<private> rules_b: array<u32, 24>;

fn tally(slot: u32, counts: u32) {
    let g = counts & 15u;
    let h = (counts >> 4u) & 15u;
    let y = (counts >> 8u) & 15u;
    let w = (counts >> 12u) & 15u;
    let x = (counts >> 16u) & 15u;
    let t = max(max(max(g, h), y) * 3u, max(w, x) * 5u);
    top[slot] = t;
    var mask = 0u;
    if g * 3u == t { mask |= 1u; }
    if h * 3u == t { mask |= 2u; }
    if y * 3u == t { mask |= 4u; }
    if w * 5u == t { mask |= 8u; }
    if x * 5u == t { mask |= 16u; }
    winners[slot] = mask;
}

// Bitmask of the group's plants carrying `gene` in `slot`.
fn carriers(slot: u32, gene: u32) -> u32 {
    var mask = 0u;
    for (var i = 0u; i < params.group_size; i++) {
        mask |= ((pool[ids[i] * 6u + slot] >> (4u * gene)) & 1u) << i;
    }
    return mask;
}

// Same logic as rules::planting_order: peel plants off the back of the order.
fn order_exists(picked: ptr<function, array<u32, 6>>) -> bool {
    var count = 0u;
    for (var slot = 0u; slot < 6u; slot++) {
        if countOneBits(choices[slot]) < 2u { continue; }
        let want = countTrailingZeros((*picked)[slot]);
        let mine = carriers(slot, want);
        var rivals = choices[slot] & ~(*picked)[slot];
        while rivals != 0u {
            let rival = countTrailingZeros(rivals);
            rivals &= rivals - 1u;
            rules_a[count] = mine;
            rules_b[count] = carriers(slot, rival);
            count += 1u;
        }
    }
    var left = (1u << params.group_size) - 1u;
    loop {
        if count == 0u { return true; }
        var blocked = 0u;
        for (var i = 0u; i < count; i++) { blocked |= rules_a[i]; }
        let free = left & ~blocked;
        if free == 0u { return false; }
        left &= ~free;
        var kept = 0u;
        for (var i = 0u; i < count; i++) {
            if (rules_b[i] & free) == 0u {
                rules_a[kept] = rules_a[i];
                rules_b[kept] = rules_b[i];
                kept += 1u;
            }
        }
        count = kept;
    }
    return false;
}

fn record(index: u32, centre: u32) {
    let base = index * RECIPE_STRIDE;
    recipes[base] = ids[0] | (ids[1] << 16u);
    recipes[base + 1u] = ids[2] | (ids[3] << 16u);
    recipes[base + 2u] = ids[4] | (ids[5] << 16u);
    recipes[base + 3u] = ids[6] | (ids[7] << 16u);
    recipes[base + 4u] = centre + 1u;
    recipes[base + 5u] = params.group_size;
}

fn claim(index: u32, centre: u32) {
    loop {
        let swap = atomicCompareExchangeWeak(&claims[index], WANT_FREE, CLAIMED);
        if swap.exchanged { record(index, centre); return; }
        if swap.old_value != WANT_FREE { return; }
    }
}

fn is_wanted(index: u32) -> bool {
    return ((want[6u + (index >> 5u)] >> (index & 31u)) & 1u) != 0u;
}

// Tries every combination of the slots' allowed genes.
fn emit(centre: u32) {
    var tied = 0u;
    var index = 0u;
    var options: array<u32, 6>;
    for (var slot = 0u; slot < 6u; slot++) {
        options[slot] = choices[slot] & allowed[slot];
        if options[slot] == 0u { return; }
        if countOneBits(choices[slot]) > 1u { tied += 1u; }
        index = index * 5u + countTrailingZeros(options[slot]);
    }
    if tied == 0u {
        if is_wanted(index) { claim(index, centre); }
        return;
    }
    var remaining = options;
    loop {
        var picked: array<u32, 6>;
        index = 0u;
        for (var slot = 0u; slot < 6u; slot++) {
            picked[slot] = remaining[slot] & (~remaining[slot] + 1u);
            index = index * 5u + countTrailingZeros(remaining[slot]);
        }
        if is_wanted(index) {
            // One tied slot can always be satisfied by planting the winners first.
            if tied == 1u || order_exists(&picked) { claim(index, centre); }
        }
        var slot = 6u;
        loop {
            if slot == 0u { return; }
            slot -= 1u;
            let next = remaining[slot] & (remaining[slot] - 1u);
            if next != 0u { remaining[slot] = next; break; }
            remaining[slot] = options[slot];
        }
    }
}

// True when this slot has passed any centre's weight and has no wanted winner.
fn hopeless(slot: u32) -> bool {
    return top[slot] > 5u && (winners[slot] & allowed[slot]) == 0u;
}

// Applies one centre to the group and tries the outcomes.
fn with_centre(c: u32) {
    for (var slot = 0u; slot < 6u; slot++) {
        let bit = pool[c * 6u + slot];
        // Red genes occupy the upper nibbles of the packed value.
        let weight = select(3u, 5u, bit >= 4096u);
        if weight >= top[slot] {
            choices[slot] = 1u << (countTrailingZeros(bit) / 4u);
        } else {
            choices[slot] = winners[slot];
        }
    }
    emit(c);
}

fn in_group(c: u32) -> bool {
    for (var i = 0u; i < params.group_size; i++) {
        if ids[i] == c { return true; }
    }
    return false;
}

fn leaf(c0: u32, c1: u32, c2: u32, c3: u32, c4: u32, c5: u32, spent: u32) {
    tally(0u, c0); if hopeless(0u) { return; }
    tally(1u, c1); if hopeless(1u) { return; }
    tally(2u, c2); if hopeless(2u) { return; }
    tally(3u, c3); if hopeless(3u) { return; }
    tally(4u, c4); if hopeless(4u) { return; }
    tally(5u, c5); if hopeless(5u) { return; }
    var held = 0u;
    for (var slot = 0u; slot < 6u; slot++) {
        if top[slot] <= 5u { held |= 1u << slot; }
    }
    if held == 0u {
        if spent != params.budget { return; }
        choices = winners;
        emit(NO_CENTRE);
        return;
    }
    let end = centres[held + 1u];
    for (var at = centres[held]; at < end; at++) {
        let c = centres[65u + at];
        if !in_group(c) && spent + costs[c] == params.budget { with_centre(c); }
    }
    // A clone of a group member costs nothing extra, even if it isn't the listed centre.
    if spent == params.budget {
        for (var i = 0u; i < params.group_size; i++) { with_centre(ids[i]); }
    }
}

const DISPATCH_ROW: u32 = 65535u;
const WORKGROUP: u32 = 64u;

@compute @workgroup_size(64)
fn main(
    @builtin(global_invocation_id) global: vec3<u32>,
    @builtin(workgroup_id) group: vec3<u32>,
    @builtin(local_invocation_index) local: u32,
) {
    let shared_prefix = params.pooled != 0u;
    let entry = select(global.x + global.y * DISPATCH_ROW * WORKGROUP, group.x + group.y * DISPATCH_ROW, shared_prefix);
    if entry >= params.prefix_count { return; }
    for (var slot = 0u; slot < 6u; slot++) { allowed[slot] = want[slot]; }
    let base = entry * PREFIX_STRIDE;
    let p0 = prefixes[base];
    let p1 = prefixes[base + 1u];
    let p2 = prefixes[base + 2u];
    let p3 = prefixes[base + 3u];
    let p4 = prefixes[base + 4u];
    let p5 = prefixes[base + 5u];
    let first = prefixes[base + 6u];
    let w0 = prefixes[base + 7u];
    let w1 = prefixes[base + 8u];
    let w2 = prefixes[base + 9u];
    let w3 = prefixes[base + 10u];
    let paid = prefixes[base + 11u];
    ids[0] = w0 & 0xFFFFu; ids[1] = w0 >> 16u;
    ids[2] = w1 & 0xFFFFu; ids[3] = w1 >> 16u;
    ids[4] = w2 & 0xFFFFu; ids[5] = w2 >> 16u;
    ids[6] = w3 & 0xFFFFu; ids[7] = 0u;

    let n = params.pool_len;
    let budget = params.budget;
    if shared_prefix {
        // The workgroup's threads deal the second-to-last plant out between themselves.
        let at = params.group_size - 2u;
        let stop = prefixes[base + 13u];
        for (var a = prefixes[base + 12u] + local; a < stop; a += WORKGROUP) {
            let spent_a = paid + select(costs[a], 0u, a == first);
            if spent_a > budget { break; }
            ids[at] = a;
            let i = a * 6u;
            let q0 = p0 + pool[i];
            let q1 = p1 + pool[i + 1u];
            let q2 = p2 + pool[i + 2u];
            let q3 = p3 + pool[i + 3u];
            let q4 = p4 + pool[i + 4u];
            let q5 = p5 + pool[i + 5u];
            for (var b = a; b < n; b++) {
                let spent_b = spent_a + select(costs[b], 0u, b == a);
                if spent_b > budget { break; }
                ids[at + 1u] = b;
                let j = b * 6u;
                leaf(q0 + pool[j], q1 + pool[j + 1u], q2 + pool[j + 2u],
                     q3 + pool[j + 3u], q4 + pool[j + 4u], q5 + pool[j + 5u], spent_b);
            }
        }
    } else {
        let last = params.group_size - 1u;
        let stop = prefixes[base + 13u];
        for (var a = prefixes[base + 12u]; a < stop; a++) {
            let spent_a = paid + select(costs[a], 0u, a == first);
            if spent_a > budget { break; }
            ids[last] = a;
            let i = a * 6u;
            leaf(p0 + pool[i], p1 + pool[i + 1u], p2 + pool[i + 2u],
                 p3 + pool[i + 3u], p4 + pool[i + 4u], p5 + pool[i + 5u], spent_a);
        }
    }
}
