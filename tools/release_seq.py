#!/usr/bin/env python3
"""Reserve a release number durably BEFORE signing; failures consume numbers."""
import os, re, stat, sys, tempfile

def reserve(directory):
    directory = os.path.abspath(directory)
    path = os.path.join(directory, 'SEQ')
    try:
        fd = os.open(path, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK)
    except FileNotFoundError:
        value = 0
    else:
        with os.fdopen(fd, 'rb') as source:
            meta = os.fstat(source.fileno())
            if not stat.S_ISREG(meta.st_mode) or meta.st_size > 21:
                raise ValueError('SEQ: нужен небольшой обычный файл')
            raw = source.read(22)
        if not re.fullmatch(rb'(0|[1-9][0-9]*)\n?', raw):
            raise ValueError('SEQ: нужен канонический десятичный u64')
        value = int(raw)
    if value >= 2**64 - 1:
        raise ValueError('SEQ: переполнение u64')
    value += 1
    fd, temporary = tempfile.mkstemp(prefix='.SEQ-', dir=directory)
    try:
        with os.fdopen(fd, 'wb') as target:
            target.write(f'{value}\n'.encode('ascii'))
            target.flush()
            os.fsync(target.fileno())
        os.replace(temporary, path)
        dfd = os.open(directory, os.O_RDONLY | os.O_DIRECTORY)
        try:
            os.fsync(dfd)
        finally:
            os.close(dfd)
    finally:
        if os.path.exists(temporary):
            os.unlink(temporary)
    return value

if __name__ == '__main__':
    print(reserve(sys.argv[1]))
