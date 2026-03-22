import argparse
import secrets


def main():
    parser = argparse.ArgumentParser(description="Generate a user token for Advanced Memory MCP")
    parser.add_argument("username", help="Username to generate a token for")
    args = parser.parse_args()

    random_hex = secrets.token_hex(32)
    token = f"{args.username}_{random_hex}"
    print(token)


if __name__ == "__main__":
    main()
