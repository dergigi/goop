#!/usr/bin/env python3
"""Check the shipped PE subsystem and embedded GPUI/Explorer icon without running it."""
import argparse
from pathlib import Path
import struct


def check(executable, icon):
    data = Path(executable).read_bytes()
    ico = Path(icon).read_bytes()
    assert data[:2] == b'MZ', 'Not a Windows executable'
    pe = struct.unpack_from('<I', data, 0x3c)[0]
    assert data[pe:pe + 4] == b'PE\0\0', 'Missing PE header'
    optional = pe + 24
    magic = struct.unpack_from('<H', data, optional)[0]
    assert magic in (0x10b, 0x20b), 'Unknown PE optional header'
    assert struct.unpack_from('<H', data, optional + 68)[0] == 2, 'Executable uses the console subsystem; expected Windows GUI'
    section_count = struct.unpack_from('<H', data, pe + 6)[0]
    section_start = optional + struct.unpack_from('<H', data, pe + 20)[0]
    sections = [struct.unpack_from('<8sIIII', data, section_start + i * 40) for i in range(section_count)]

    def offset(rva):
        for _, virtual_size, address, raw_size, raw in sections:
            if address <= rva < address + max(virtual_size, raw_size):
                return raw + rva - address
        raise AssertionError(f'Unmapped resource RVA: {rva:x}')

    directories = optional + (112 if magic == 0x20b else 96)
    resource_rva, resource_size = struct.unpack_from('<II', data, directories + 2 * 8)
    assert resource_rva and resource_size, 'Executable has no Windows resources'
    base = offset(resource_rva)

    def entries(relative):
        at = base + relative
        named, ids = struct.unpack_from('<HH', data, at + 12)
        return dict(struct.unpack_from('<II', data, at + 16 + i * 8) for i in range(named + ids))

    def resource(kind, identifier):
        kinds = entries(0)
        assert kind in kinds and kinds[kind] & 0x80000000, f'Missing resource type {kind}'
        names = entries(kinds[kind] & 0x7fffffff)
        assert identifier in names and names[identifier] & 0x80000000, f'Missing resource ID {identifier}'
        languages = entries(names[identifier] & 0x7fffffff)
        assert languages, 'Resource has no language entry'
        leaf = next(iter(languages.values()))
        assert not leaf & 0x80000000, 'Unexpected resource directory'
        rva, size = struct.unpack_from('<II', data, base + leaf)
        return data[offset(rva):offset(rva) + size]

    group = resource(14, 1)  # RT_GROUP_ICON, the ID loaded by GPUI.
    reserved, kind, count = struct.unpack_from('<HHH', ico)
    assert reserved == 0 and kind == 1 and count > 0, 'Invalid source ICO'
    assert struct.unpack_from('<HHH', group) == (0, 1, count), 'Embedded icon image count differs'
    for index in range(count):
        source = struct.unpack_from('<BBBBHHII', ico, 6 + index * 16)
        embedded = struct.unpack_from('<BBBBHHIH', group, 6 + index * 14)
        assert source[:7] == embedded[:7], 'Embedded icon size/format differs'
        expected = ico[source[7]:source[7] + source[6]]
        assert resource(3, embedded[7]) == expected, 'Embedded icon pixels differ from Goop icon'
    print(f'{executable}: Windows GUI subsystem and {count} Goop icon images verified.')


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('executable')
    parser.add_argument('--icon', default='desktop/resources/icon.ico')
    args = parser.parse_args()
    check(args.executable, args.icon)
