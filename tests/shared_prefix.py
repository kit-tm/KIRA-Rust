import argparse


def shared_xor_prefix_bits(hex1: str, hex2: str) -> int:
    val1 = int(hex1, 16)
    val2 = int(hex2, 16)

    xor_result = val1 ^ val2

    if xor_result == 0:
        return len(hex1) * 4

    total_bits = len(hex1) * 4
    return total_bits - xor_result.bit_length()


def main():
    parser = argparse.ArgumentParser(
        description="Calculate the common XOR prefix length (in bits) between two hex strings."
    )

    # Positional arguments
    parser.add_argument("hex1", help="First hex string (e.g., 'a0f2')")
    parser.add_argument("hex2", help="Second hex string (e.g., 'b1e3')")

    # Optional flag for verbose output
    parser.add_argument("-v", "--verbose", action="store_true", help="Include binary visualization")

    args = parser.parse_args()

    try:
        prefix = shared_xor_prefix_bits(args.hex1, args.hex2)

        if args.verbose:
            # Show the bits to verify the prefix
            b1 = bin(int(args.hex1, 16))[2:].zfill(len(args.hex1) * 4)
            b2 = bin(int(args.hex2, 16))[2:].zfill(len(args.hex2) * 4)
            print(f"Hex 1 Bits: {b1}")
            print(f"Hex 2 Bits: {b2}")
            print("-" * (len(b1) + 12))

        print(f"Shared XOR Prefix: {prefix} bits")

    except ValueError:
        print("Error: Invalid hex string provided.")
        exit(1)


if __name__ == "__main__":
    main()
