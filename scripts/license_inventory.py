"""Collect license metadata/text for locked dependencies; build helper, Python 3.11+."""
from pathlib import Path
import argparse, csv, json, re, shutil, tomllib

ap=argparse.ArgumentParser();ap.add_argument('--registry-path',type=Path,required=True);ap.add_argument('--output',type=Path,required=True)
args=ap.parse_args();project=Path(__file__).resolve().parent.parent;output=args.output.resolve();output.mkdir(parents=True,exist_ok=True)
rows=[];seen=set()

def collect(ecosystem,name,version,license_name,location):
    key=(ecosystem,name,version)
    if key in seen:return
    seen.add(key);dest=output/ecosystem/re.sub(r'[^A-Za-z0-9._-]','_',name+'-'+version)
    texts=[]
    for p in sorted(location.iterdir()):
        if p.is_file() and p.name.lower().startswith(('license','licence','copying','notice','copyright')):
            dest.mkdir(parents=True,exist_ok=True);shutil.copyfile(p,dest/p.name);texts.append((dest/p.name).relative_to(output).as_posix())
    rows.append({'ecosystem':ecosystem,'name':name,'version':version,'license':str(license_name or 'unspecified'),'texts':texts})

lock=tomllib.loads((project/'src-tauri/Cargo.lock').read_text(encoding='utf-8'))
for package in lock.get('package',[]):
    if not package.get('source','').startswith('registry+'):continue
    matches=list(args.registry_path.glob('*/'+package['name']+'-'+package['version']))
    if not matches:raise RuntimeError('Missing locked dependency source: '+package['name'])
    location=matches[0];meta=tomllib.loads((location/'Cargo.toml').read_text(encoding='utf-8')).get('package',{})
    collect('rust',package['name'],package['version'],meta.get('license'),location)

store=project/'node_modules/.pnpm'
for version_folder in sorted(store.iterdir()):
    modules=version_folder/'node_modules'
    if not modules.is_dir():continue
    candidates=[]
    for p in modules.iterdir():
        if p.name.startswith('@') and p.is_dir():candidates.extend(p.iterdir())
        else:candidates.append(p)
    for p in candidates:
        manifest=p/'package.json'
        if not manifest.is_file():continue
        meta=json.loads(manifest.read_text(encoding='utf-8'));name=meta.get('name');version=meta.get('version')
        if name and version:collect('npm',name,version,meta.get('license'),p)

rows.sort(key=lambda r:(r['ecosystem'],r['name'],r['version']))
(output/'inventory.json').write_text(json.dumps(rows,indent=2,ensure_ascii=False),encoding='utf-8')
with (output/'inventory.csv').open('w',encoding='utf-8-sig',newline='') as f:
    writer=csv.DictWriter(f,fieldnames=['ecosystem','name','version','license','texts']);writer.writeheader()
    for row in rows:writer.writerow({**row,'texts':';'.join(row['texts'])})
(output/'README.txt').write_text('Dependency license inventory for this candidate. Includes locked build/test/platform dependencies, not a claim that all are linked into the executable. Original license notices are retained below. Application license/public publication is a separate owner decision.\n',encoding='utf-8')
print(json.dumps({'packages':len(rows),'rust':sum(r['ecosystem']=='rust' for r in rows),'npm':sum(r['ecosystem']=='npm' for r in rows),'withoutText':sum(not r['texts'] for r in rows)}))
