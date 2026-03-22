import argparse
import json
import secrets
from pathlib import Path


def main():
    parser = argparse.ArgumentParser(description="Generate a user token for Advanced Memory MCP")
    parser.add_argument("username", help="Username to generate a token for")
    parser.add_argument("--config", help="Path to tokens.json to append the new token to")
    parser.add_argument("--file", help="Logical data file name (used with --config)")
    parser.add_argument("--mode", choices=["read-write", "read-only"], default="read-write", help="Permission mode (default: read-write, used with --config)")
    args = parser.parse_args()

    if args.config and not args.file:
        parser.error("--file is required when using --config")

    random_hex = secrets.token_hex(32)
    token = f"{args.username}_{random_hex}"

    if args.config:
        config_path = Path(args.config)
        if config_path.exists():
            with open(config_path) as f:
                config = json.load(f)
        else:
            config = {}

        config[token] = {"file": args.file, "mode": args.mode}

        with open(config_path, "w") as f:
            json.dump(config, f, indent=2)
            f.write("\n")

        print(f"Token added to {args.config}")

    print(token)


if __name__ == "__main__":
    main()
