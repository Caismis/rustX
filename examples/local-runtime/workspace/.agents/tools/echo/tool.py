"""A dependency-free custom Python tool for the rustX local-runtime example."""


def main(arguments):
    """Return the caller's message as a JSON-serializable object."""
    return {"message": arguments["message"]}
