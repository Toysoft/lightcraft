"""Original data-only compiler for a pinned Lensfun CC BY-SA 3.0 database.

No Lensfun library or calibration-tool code is used. Run with the downloaded
source directory (containing data/db and manifest.json) and an output JSON path.
All XML bytes must match the input manifest before compilation.
"""
import argparse
import hashlib
import json
import math
from pathlib import Path
import xml.etree.ElementTree as ET


# Public XML parameters; refuse new coordinate/model terms until the evaluator supports them.
PARAMETERS = {
    ('distortion', 'ptlens'): {'focal', 'real-focal', 'a', 'b', 'c'},
    ('distortion', 'poly3'): {'focal', 'real-focal', 'k1'},
    ('distortion', 'poly5'): {'focal', 'real-focal', 'k1', 'k2'},
    ('tca', 'linear'): {'focal', 'kr', 'kb'},
    ('tca', 'poly3'): {'focal', 'vr', 'cr', 'br', 'vb', 'cb', 'bb'},
    ('vignetting', 'pa'): {'focal', 'aperture', 'distance', 'k1', 'k2', 'k3'},
}


def number(s):
    if ':' in s:
        a, b = s.split(':')
        value = float(a) / float(b)
    else:
        value = float(s)
    if not math.isfinite(value):
        raise ValueError('Non-finite calibration value: ' + s)
    return value


def compile_catalog(root):
    manifest = json.loads((root / 'manifest.json').read_text())
    cameras, lenses, mounts = [], [], []
    for entry in manifest['files']:
        if not entry['path'].startswith('data/db/') or not entry['path'].endswith('.xml'):
            continue
        content = (root / entry['path']).read_bytes()
        if hashlib.sha256(content).hexdigest() != entry['sha256']:
            raise ValueError('SHA-256 mismatch: ' + entry['path'])
        doc = ET.fromstring(content)
        if doc.attrib.get('version') != '2':
            raise ValueError('Unsupported database schema: ' + entry['path'])
        for cam in doc.findall('camera'):
            if cam.find('variant') is not None:
                continue
            cameras.append(dict(make=cam.findtext('maker', ''), model=cam.findtext('model', ''),
                                crop=number(cam.findtext('cropfactor', '0')), mount=cam.findtext('mount', '')))
        for mount in doc.findall('mount'):
            mounts.append(dict(name=mount.findtext('name', ''), compatible=[c.text for c in mount.findall('compat')]))
        for lens in doc.findall('lens'):
            if lens.findtext('type', 'rectilinear') != 'rectilinear':
                continue
            if lens.find('center') is not None:
                # Decentered and projection-changing lenses need a separate coordinate model.
                continue
            samples = []
            for calibration in lens.findall('calibration'):
                if calibration.attrib:
                    raise ValueError('Unsupported calibration attributes: ' + entry['path'])
                for kind in ('distortion', 'tca', 'vignetting'):
                    for point in calibration.findall(kind):
                        model = point.attrib.get('model')
                        if model in ('none', 'acm'):
                            continue
                        allowed = PARAMETERS.get((kind, model))
                        if allowed is None:
                            raise ValueError('Unsupported calibration model: ' + entry['path'])
                        attr = dict(point.attrib)
                        attr.pop('model')
                        if not set(attr) <= allowed:
                            raise ValueError('Unsupported calibration parameters: ' + entry['path'])
                        samples.append(dict(kind=kind, model=model, values={k: number(v) for k, v in attr.items()}))
            if samples:
                lenses.append(dict(make=lens.findtext('maker', ''), model=lens.findtext('model', ''),
                                   names=[n.text for n in lens.findall('model')],
                                   mounts=[n.text for n in lens.findall('mount')],
                                   crop=number(lens.findtext('cropfactor', '0')),
                                   aspect=number(lens.findtext('aspect-ratio', '1.5')), samples=samples))
    return dict(source=manifest['repository'], revision=manifest['commit'], license='CC-BY-SA-3.0',
                cameras=cameras, mounts=mounts, lenses=lenses)


if __name__ == '__main__':
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument('source', type=Path)
    ap.add_argument('output', type=Path)
    args = ap.parse_args()
    result = compile_catalog(args.source)
    args.output.write_text(json.dumps(result, separators=(',', ':'), sort_keys=True) + '\n')
    print(len(result['cameras']), 'cameras;', len(result['lenses']), 'rectilinear lens calibration sets')
